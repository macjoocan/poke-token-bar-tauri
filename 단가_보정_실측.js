// 단가 보정 실측 — 지금 앱 단가 vs 공식 단가 비교
//
//   node 단가_보정_실측.js
//
// 로컬 Claude 로그(~/.claude/projects/**.jsonl)를 전부 읽어, 같은 데이터에
// 두 단가 테이블을 적용해 금액 차이를 낸다. 자세한 배경은 단가_보정_작업지시.md.
//
// 읽기 전용이다. 아무것도 쓰지 않는다.
//
// 주의: 실행 시점의 로그 전체를 읽으므로 절대 금액은 계속 커진다.
//       "현재 앱" 열이 지금 앱 수치와 맞는지(스크립트 검증), 고친 뒤에는
//       "공식 단가" 열이 앱 수치와 맞는지(작업 검증)를 본다.

const fs = require('fs');
const path = require('path');
const os = require('os');

const root = path.join(os.homedir(), '.claude', 'projects');

// ---- 수집 (dedup keep-max) --------------------------------------------------
// 같은 응답이 스트리밍·재개로 여러 번 기록되고 output 이 점점 커진다.
// (message.id | requestId) 별로 합이 가장 큰 것만 남긴다. M0 검증 문서 참조.

const seen = new Map();
const n = v => (typeof v === 'number' && isFinite(v) ? v : 0);

function walk(dir) {
  let items;
  try { items = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
  for (const it of items) {
    const p = path.join(dir, it.name);
    if (it.isDirectory()) { walk(p); continue; }
    if (!it.name.endsWith('.jsonl')) continue;
    let txt;
    try { txt = fs.readFileSync(p, 'utf8'); } catch { continue; }
    for (const line of txt.split('\n')) {
      // fast-path: 두 리터럴이 다 있는 줄만 파싱 (전체 파싱은 너무 느리다)
      if (!line || line.indexOf('"usage"') < 0 || line.indexOf('"assistant"') < 0) continue;
      let o;
      try { o = JSON.parse(line); } catch { continue; }
      if (o.type !== 'assistant' || !o.message || !o.message.usage) continue;
      const model = o.message.model || 'unknown';
      if (model === '<synthetic>') continue;          // 실제 호출이 아니다
      const u = o.message.usage, cc = u.cache_creation || {};
      const r = {
        model,
        input: n(u.input_tokens),
        output: n(u.output_tokens),
        cw: n(u.cache_creation_input_tokens),
        cr: n(u.cache_read_input_tokens),
        c1h: n(cc.ephemeral_1h_input_tokens),
        c5m: n(cc.ephemeral_5m_input_tokens),
      };
      const tot = r.input + r.output + r.cw + r.cr;
      const key = (o.message.id || '') + '|' + (o.requestId || '');
      const prev = seen.get(key);
      if (prev && prev.tot >= tot) continue;
      seen.set(key, { tot, r });
    }
  }
}

// ---- 지금 앱 단가 (model_pricing.rs 현재 상태를 그대로 옮긴 것) --------------
// cache_write 단가가 하나뿐이고, 그 값은 5분 단가다.

function poke(m) {
  const P = (i, o, w, rd) => ({ i, o, w, rd });
  if (m === 'claude-opus-4-8' || m === 'claude-opus-4-7') return P(5, 25, 6.25, 0.5);
  if (m === 'claude-sonnet-4-6') return P(3, 15, 3.75, 0.3);
  if (m === 'claude-haiku-4-5-20251001') return P(1, 5, 1.25, 0.1);
  if (m === 'claude-fable-5') return P(0, 0, 0, 0);       // 명시적 $0
  const L = m.toLowerCase();
  if (L.includes('opus')) return P(5, 25, 6.25, 0.5);
  if (L.includes('sonnet')) return P(3, 15, 3.75, 0.3);
  if (L.includes('haiku')) return P(1, 5, 1.25, 0.1);
  if (L.includes('gpt') || L.includes('codex') || L.includes('o4') || L.includes('o3'))
    return P(5, 30, 0, 0.5);
  return P(0, 0, 0, 0);                                   // fable-5-1 등 → $0
}

// ---- 공식 단가 (2026-09-07, platform.claude.com/docs/en/about-claude/pricing) --
// 1시간 / 5분 캐시 쓰기가 갈린다. 캐시 읽기는 0.1배, 단 Fable/Mythos 5.1 은 0.025배.

function real(m) {
  const P = (i, o, w5, w1, rd) => ({ i, o, w5, w1, rd });
  if (m === 'claude-fable-5-1' || m === 'claude-mythos-5-1') return P(10, 50, 12.5, 20, 0.25);
  if (m === 'claude-fable-5' || m === 'claude-mythos-5') return P(10, 50, 12.5, 20, 1);
  if (/^claude-opus-(5|4-8|4-7|4-6|4-5)/.test(m)) return P(5, 25, 6.25, 10, 0.5);
  if (/^claude-opus-4(-1)?$/.test(m)) return P(15, 75, 18.75, 30, 1.5);
  if (m === 'claude-sonnet-5') return P(2, 10, 2.5, 4, 0.2);
  if (/^claude-sonnet-4(-6|-5)?/.test(m)) return P(3, 15, 3.75, 6, 0.3);
  if (/^claude-haiku-4-5/.test(m)) return P(1, 5, 1.25, 2, 0.1);
  if (/^claude-haiku-3-5/.test(m)) return P(0.8, 4, 1, 1.6, 0.08);
  return null;                                            // 모르면 계산에서 뺀다
}

walk(root);

// ---- 집계 ------------------------------------------------------------------

const M = 1e6;
const agg = new Map();
const unknown = new Set();

for (const { r } of seen.values()) {
  let a = agg.get(r.model);
  if (!a) { a = { calls: 0, pk: 0, rl: 0, cwPk: 0, cwRl: 0, tok: 0 }; agg.set(r.model, a); }
  a.calls++;
  a.tok += r.input + r.output + r.cw + r.cr;

  const p = poke(r.model);
  a.cwPk += (r.cw * p.w) / M;
  a.pk += (r.input * p.i + r.output * p.o + r.cw * p.w + r.cr * p.rd) / M;

  const q = real(r.model);
  if (!q) { unknown.add(r.model); continue; }
  // cache_creation 이 없는 옛 로그 호환: 잔여분은 5분으로 본다 (작업지시서 3절과 동일 규칙)
  const rest = Math.max(0, r.cw - r.c1h - r.c5m);
  const cwCost = (r.c1h * q.w1 + (r.c5m + rest) * q.w5) / M;
  a.cwRl += cwCost;
  a.rl += (r.input * q.i + r.output * q.o + r.cr * q.rd) / M + cwCost;
}

// ---- 출력 ------------------------------------------------------------------

const $ = v => '$' + v.toFixed(2);
const T = v => (v / M).toFixed(2) + 'M';
const rows = [...agg.entries()].sort((a, b) => b[1].rl - a[1].rl);

console.log('\n중복 제거 후 응답 ' + seen.size.toLocaleString() + '건\n');
console.log('모델'.padEnd(30) + '현재 앱'.padStart(14) + '공식 단가'.padStart(14)
          + '차이'.padStart(13) + '   캐시쓰기 (현재 → 공식)');
console.log(''.padEnd(95, '-'));

let tp = 0, tr = 0;
for (const [m, a] of rows) {
  tp += a.pk; tr += a.rl;
  console.log(m.padEnd(30) + $(a.pk).padStart(14) + $(a.rl).padStart(14)
    + $(a.rl - a.pk).padStart(13) + '   ' + $(a.cwPk) + ' → ' + $(a.cwRl));
}
console.log(''.padEnd(95, '-'));
console.log('합계'.padEnd(30) + $(tp).padStart(14) + $(tr).padStart(14) + $(tr - tp).padStart(13));
console.log('\n과소집계: ' + $(tr - tp) + ' (' + ((1 - tp / tr) * 100).toFixed(1) + '%)');

if (unknown.size) {
  console.log('\n공식 단가 표에 없어 계산에서 제외한 모델:');
  for (const m of unknown) console.log('  ' + m);
}

console.log('\n모델 구성 (중복 제거 후):');
console.log('모델'.padEnd(30) + '응답'.padStart(8) + '입력'.padStart(10) + '출력'.padStart(10)
          + '캐시쓰기'.padStart(11) + '캐시읽기'.padStart(12) + '1h'.padStart(10) + '5m'.padStart(10));
const cen = new Map();
for (const { r } of seen.values()) {
  let c = cen.get(r.model);
  if (!c) { c = { calls: 0, input: 0, output: 0, cw: 0, cr: 0, c1h: 0, c5m: 0 }; cen.set(r.model, c); }
  c.calls++; c.input += r.input; c.output += r.output;
  c.cw += r.cw; c.cr += r.cr; c.c1h += r.c1h; c.c5m += r.c5m;
}
for (const [m, c] of [...cen].sort((a, b) => b[1].cr - a[1].cr)) {
  console.log(m.padEnd(30) + String(c.calls).padStart(8) + T(c.input).padStart(10)
    + T(c.output).padStart(10) + T(c.cw).padStart(11) + T(c.cr).padStart(12)
    + T(c.c1h).padStart(10) + T(c.c5m).padStart(10));
}
console.log();
