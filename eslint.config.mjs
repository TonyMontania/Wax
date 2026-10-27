// Checks the interface's scripts: npx eslint@10 ui
// The scripts in ui/ are plain <script>s sharing one global scope (engine.js → app.js →
// extras.js → party.js → more.js), so what one declares at the top level is known to the others.
import { readFileSync, readdirSync } from 'node:fs';

const UI = 'ui';
const scripts = readdirSync(UI).filter(f => f.endsWith('.js'));

// Top-level declarations of every script: `function x`, `class x`, `const/let/var x` (and `const a = …, b = …`)
const shared = {};
for (const f of scripts) {
  for (const line of readFileSync(`${UI}/${f}`, 'utf8').split('\n')) {
    let m = line.match(/^(?:async\s+)?(?:function\*?|class)\s+([A-Za-z_$][\w$]*)/);
    if (m) { shared[m[1]] = 'writable'; continue; }
    m = line.match(/^(?:const|let|var)\s+(.*)/);
    if (!m) continue;
    // Names declared on this line (skip what's inside brackets / strings / templates)
    let depth = 0, quote = '', rest = '';
    for (const c of m[1]) {
      if (quote) { if (c === quote) quote = ''; continue; }
      if ('`\'"'.includes(c)) { quote = c; continue; }
      if ('([{'.includes(c)) depth++;
      else if (')]}'.includes(c)) depth--;
      else if (depth === 0) rest += c;
    }
    for (const part of rest.split(',')) {
      const n = part.trim().match(/^([A-Za-z_$][\w$]*)\s*(=|;|$)/);
      if (n) shared[n[1]] = 'writable';
    }
  }
}

const browser = Object.fromEntries([
  'window', 'document', 'navigator', 'location', 'history', 'console', 'performance', 'localStorage', 'sessionStorage',
  'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'requestAnimationFrame', 'cancelAnimationFrame', 'queueMicrotask',
  'addEventListener', 'removeEventListener', 'dispatchEvent', 'getComputedStyle', 'matchMedia', 'fetch', 'URL', 'URLSearchParams',
  'Event', 'CustomEvent', 'EventTarget', 'MouseEvent', 'KeyboardEvent', 'Audio', 'Image', 'Blob', 'File', 'FileReader', 'FormData',
  'IntersectionObserver', 'ResizeObserver', 'MutationObserver', 'DOMParser', 'CSS', 'Intl', 'TextEncoder', 'TextDecoder', 'crypto',
  'atob', 'btoa', 'alert', 'confirm', 'structuredClone', 'HTMLElement', 'Element', 'Node', 'getSelection', 'innerWidth', 'innerHeight',
  'devicePixelRatio', 'screen', 'AbortController', 'WebSocket', 'Worker', 'OffscreenCanvas', 'createImageBitmap', 'scrollTo',
].map(n => [n, 'readonly']));

export default [
  {
    files: ['ui/**/*.js'],
    languageOptions: { ecmaVersion: 2024, sourceType: 'script', globals: { ...browser, ...shared } },
    rules: {
      'no-undef': 'error',
      'no-unused-vars': ['warn', { vars: 'local', args: 'none', caughtErrors: 'none' }],
      'no-dupe-keys': 'error',
      'no-duplicate-case': 'error',
      'no-unreachable': 'error',
      'no-const-assign': 'error',
      'no-redeclare': 'off', // the shared scope is built from these very files
    },
  },
];
