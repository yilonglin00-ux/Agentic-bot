#!/usr/bin/env node
'use strict';
const assert = require('assert');
const markdown = require('../markdown.js');

function all(ast, tag) {
  const found = [];
  (function visit(items) {
    for (const item of items || []) {
      if (item.type === 'element') {
        if (item.tag === tag) found.push(item);
        visit(item.children);
      }
    }
  }(ast));
  return found;
}
function words(ast) {
  let result = '';
  (function visit(items) { for (const item of items || []) item.type === 'text' ? result += item.value : visit(item.children); }(ast));
  return result;
}

// A–J: semantic structure, including the deliberate <u> convention.
let ast = markdown.parse('**Fett**\n\n*kursiv*\n\n### Überschrift\n\n- eins\n  - verschachtelt\n- zwei\n\n1. erster\n2. zweiter\n\n`x = 1`\n\n```js\nconst x = 1;\n```\n\n> Zitat\n\n[OpenAI](https://openai.com)\n\n<u>unterstrichen</u>');
assert.equal(all(ast, 'strong').length, 1, 'bold becomes strong');
assert.equal(all(ast, 'em').length, 1, 'italic becomes em');
assert.equal(all(ast, 'h3').length, 1, 'heading becomes h3');
assert.equal(all(ast, 'ul').length, 2, 'nested unordered list remains structural');
assert.equal(all(ast, 'ol').length, 1, 'ordered list becomes ol');
assert.equal(all(ast, 'code').length >= 2, true, 'inline and fenced code are code elements');
assert.equal(all(ast, 'pre').length, 1, 'fenced code becomes pre');
assert.equal(all(ast, 'blockquote').length, 1, 'quote becomes blockquote');
assert.equal(all(ast, 'a').length, 1, 'safe link becomes anchor');
assert.equal(all(ast, 'u').length, 1, 'only explicit underline convention is accepted');
assert.equal(words(ast).includes('**Fett**'), false, 'Markdown delimiters are not content');

// K: untrusted HTML and unsafe schemes are removed/never represented as DOM tags.
ast = markdown.parse('<script>globalThis.pwned=1</script><img src=x onerror=alert(1)> [bad](javascript:alert(1)) <iframe src="https://evil"></iframe>');
assert.equal(all(ast, 'script').length + all(ast, 'img').length + all(ast, 'iframe').length, 0, 'dangerous elements have no AST representation');
assert.equal(all(ast, 'a').length, 0, 'javascript URL is not linked');
assert.equal(words(ast).includes('pwned'), false, 'dangerous element body is removed');

// L/M: partial streaming source is harmless and completed source formats correctly.
assert.doesNotThrow(() => markdown.parse('**Hallo'));
assert.equal(all(markdown.parse('**Hallo**'), 'strong').length, 1);

// N: multi-section scale is linear enough for a chat update and preserves headings.
const large = Array.from({ length: 90 }, (_, i) => `## Abschnitt ${i + 1}\n\n**Kernpunkt.** ${'Inhalt mit Kontext. '.repeat(80)}`).join('\n\n');
const started = Date.now(), largeAst = markdown.parse(large), elapsed = Date.now() - started;
assert.equal(all(largeAst, 'h2').length, 90);
assert.ok(elapsed < 1500, `large response parsed in ${elapsed}ms`);
console.log(`markdown-renderer: 14 assertions passed (${elapsed}ms large parse)`);
