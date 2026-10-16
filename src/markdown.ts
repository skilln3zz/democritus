import MarkdownIt from 'markdown-it';
import hljs from 'highlight.js/lib/common';
import DOMPurify from 'dompurify';

// Raw HTML is escaped, images are disabled (they would trigger network
// requests), and the output is sanitized again before it reaches the DOM.
const md = new MarkdownIt({ html: false, linkify: false, typographer: false });
md.disable('image');

md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  const lang = (token.info.trim().split(/\s+/)[0] ?? '').toLowerCase();
  const known = lang !== '' && hljs.getLanguage(lang) !== undefined;
  const body = known
    ? hljs.highlight(token.content, { language: lang, ignoreIllegals: true }).value
    : md.utils.escapeHtml(token.content);
  const label = md.utils.escapeHtml(lang || 'text');
  return (
    `<div class="code-block"><div class="code-header"><span class="code-lang">${label}</span>` +
    `<button type="button" class="copy-btn">Copy</button></div>` +
    `<pre><code class="hljs">${body}</code></pre></div>`
  );
};

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A') {
    // Links never navigate (see main.ts); show the target on hover instead.
    node.setAttribute('title', node.getAttribute('href') ?? '');
    node.removeAttribute('target');
  }
});

export function renderMarkdown(src: string): string {
  return DOMPurify.sanitize(md.render(src), { USE_PROFILES: { html: true } });
}

export function escapeHtml(s: string): string {
  return md.utils.escapeHtml(s);
}
