// Line diff (LCS) for reviewing memory changes. Memory files are small, so the
// O(n*m) table is fine.

export interface DiffLine {
  kind: 'same' | 'add' | 'del' | 'gap';
  text: string;
}

export function diffLines(before: string, after: string, context = 2): DiffLine[] {
  const a = before.split('\n');
  const b = after.split('\n');
  const n = a.length;
  const m = b.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const all: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      all.push({ kind: 'same', text: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      all.push({ kind: 'del', text: a[i++] });
    } else {
      all.push({ kind: 'add', text: b[j++] });
    }
  }
  while (i < n) all.push({ kind: 'del', text: a[i++] });
  while (j < m) all.push({ kind: 'add', text: b[j++] });

  // Keep only changed lines plus a little context.
  const keep = all.map(() => false);
  all.forEach((l, idx) => {
    if (l.kind !== 'same') for (let k = Math.max(0, idx - context); k <= Math.min(all.length - 1, idx + context); k++) keep[k] = true;
  });
  const out: DiffLine[] = [];
  let skipped = false;
  all.forEach((l, idx) => {
    if (keep[idx]) {
      out.push(l);
      skipped = false;
    } else if (!skipped) {
      out.push({ kind: 'gap', text: '…' });
      skipped = true;
    }
  });
  return out;
}
