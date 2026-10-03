/* Noki assistant Markdown: a deliberately small, DOM-only renderer.
 * Model output never reaches innerHTML.  The parser only creates elements from
 * this whitelist, so markup, event attributes and unsafe URLs cannot execute.
 */
(function (root, factory) {
  var api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  if (root) root.NokiMarkdown = api;
}(typeof window !== 'undefined' ? window : this, function () {
  'use strict';

  function cleanSource(value) {
    var text = String(value == null ? '' : value).replace(/\r/g, '');
    // Raw HTML is not a Markdown feature we support.  Remove dangerous whole
    // elements first, then tags/attributes; <u> is handled as an explicit,
    // attribute-free convention below.
    text = text.replace(/<(script|style|iframe|object|embed|svg|math)\b[^>]*>[\s\S]*?<\/\1\s*>/gi, '');
    return text.replace(/<\/?(?!u\s*>)[A-Za-z][^>]*>/g, '');
  }

  function safeUrl(raw) {
    var value = String(raw || '').trim();
    if (!value || /[\u0000-\u001f\u007f]/.test(value)) return '';
    if (!/^(https?:|mailto:)/i.test(value)) return '';
    try {
      var url = new URL(value);
      if (url.protocol !== 'https:' && url.protocol !== 'http:' && url.protocol !== 'mailto:') return '';
      return url.href;
    } catch (_) { return ''; }
  }

  function textNode(value) { return { type: 'text', value: String(value) }; }
  function node(tag, children, attrs) { return { type: 'element', tag: tag, children: children || [], attrs: attrs || {} }; }
  function appendText(target, value) {
    if (!value) return;
    var last = target[target.length - 1];
    if (last && last.type === 'text') last.value += value;
    else target.push(textNode(value));
  }

  function inline(source) {
    var text = cleanSource(source), out = [], i = 0;
    function wrapped(marker, tag) {
      if (text.slice(i, i + marker.length) !== marker) return false;
      var end = text.indexOf(marker, i + marker.length);
      if (end <= i + marker.length) return false;
      out.push(node(tag, inline(text.slice(i + marker.length, end)))); i = end + marker.length; return true;
    }
    while (i < text.length) {
      if (text.charAt(i) === '\\' && i + 1 < text.length) { appendText(out, text.charAt(i + 1)); i += 2; continue; }
      if (text.charAt(i) === '\n') { out.push(node('br')); i++; continue; }
      if (text.slice(i, i + 3) === '<u>') {
        var uend = text.indexOf('</u>', i + 3);
        if (uend >= 0) { out.push(node('u', inline(text.slice(i + 3, uend)))); i = uend + 4; continue; }
      }
      if (text.charAt(i) === '`') {
        var codeEnd = text.indexOf('`', i + 1);
        if (codeEnd > i + 1) { out.push(node('code', [textNode(text.slice(i + 1, codeEnd))])); i = codeEnd + 1; continue; }
      }
      if (text.charAt(i) === '[') {
        var close = text.indexOf('](', i + 1), endLink = close < 0 ? -1 : text.indexOf(')', close + 2);
        if (close > i + 1 && endLink > close + 2) {
          var label = text.slice(i + 1, close), href = safeUrl(text.slice(close + 2, endLink));
          if (href) out.push(node('a', inline(label), { href: href }));
          else appendText(out, label);
          i = endLink + 1; continue;
        }
      }
      if (wrapped('***', 'strong-em') || wrapped('___', 'strong-em') || wrapped('**', 'strong') || wrapped('__', 'strong') || wrapped('~~', 'del')) continue;
      var ch = text.charAt(i);
      if ((ch === '*' || ch === '_') && wrapped(ch, 'em')) continue;
      appendText(out, ch); i++;
    }
    return out;
  }

  function isFence(line) { return /^(\s*)(`{3,}|~{3,})/.exec(line); }
  function listMatch(line) { return /^(\s*)([-+*]|\d+[.)])\s+(.*)$/.exec(line); }
  function blockStart(line) { return /^(?:\s*#{1,3}\s+|\s*>|\s*(?:[-+*]|\d+[.)])\s+|\s*(?:---+|\*\*\*+|___+)\s*$)/.test(line) || !!isFence(line); }

  function parseList(lines, start, baseIndent) {
    var first = listMatch(lines[start]), ordered = /^\d/.test(first[2]), list = node(ordered ? 'ol' : 'ul'), i = start;
    while (i < lines.length) {
      var m = listMatch(lines[i]);
      if (!m || m[1].length !== baseIndent || /^\d/.test(m[2]) !== ordered) break;
      var li = node('li', inline(m[3])); i++;
      while (i < lines.length && !lines[i].trim()) i++;
      if (i < lines.length) {
        var child = listMatch(lines[i]);
        if (child && child[1].length > baseIndent) {
          var parsed = parseList(lines, i, child[1].length); li.children.push(parsed.value); i = parsed.next;
        }
      }
      list.children.push(li);
    }
    return { value: list, next: i };
  }

  function parse(markdown) {
    var lines = cleanSource(markdown).split('\n'), out = [], i = 0;
    while (i < lines.length) {
      if (!lines[i].trim()) { i++; continue; }
      var fence = isFence(lines[i]);
      if (fence) {
        var marker = fence[2], language = lines[i].slice(fence[0].length).trim(), body = []; i++;
        while (i < lines.length && lines[i].indexOf(marker) !== 0) body.push(lines[i++]);
        if (i < lines.length) i++;
        out.push(node('pre', [node('code', [textNode(body.join('\n'))], language ? { language: language.replace(/[^\w+-]/g, '') } : {})])); continue;
      }
      var heading = /^\s*(#{1,3})\s+(.+?)\s*#*\s*$/.exec(lines[i]);
      if (heading) { out.push(node('h' + heading[1].length, inline(heading[2]))); i++; continue; }
      if (/^\s*(?:---+|\*\*\*+|___+)\s*$/.test(lines[i])) { out.push(node('hr')); i++; continue; }
      if (/^\s*>/.test(lines[i])) {
        var quote = [];
        while (i < lines.length && /^\s*>/.test(lines[i])) quote.push(lines[i++].replace(/^\s*>\s?/, ''));
        out.push(node('blockquote', parse(quote.join('\n')))); continue;
      }
      var item = listMatch(lines[i]);
      if (item) { var parsed = parseList(lines, i, item[1].length); out.push(parsed.value); i = parsed.next; continue; }
      var paragraph = [];
      while (i < lines.length && lines[i].trim() && !blockStart(lines[i])) paragraph.push(lines[i++]);
      if (paragraph.length) out.push(node('p', inline(paragraph.join('\n')))); else i++;
    }
    return out;
  }

  function materialize(doc, ast, openUrl) {
    var fragment = doc.createDocumentFragment();
    function make(item) {
      if (item.type === 'text') return doc.createTextNode(item.value);
      var el = doc.createElement(item.tag === 'strong-em' ? 'strong' : item.tag);
      if (item.tag === 'strong-em') el.className = 'ni-md-strong-em';
      if (item.tag === 'a') {
        el.href = item.attrs.href; el.rel = 'noreferrer noopener'; el.target = '_blank';
        if (openUrl) el.addEventListener('click', function (event) { event.preventDefault(); openUrl(item.attrs.href); });
      }
      if (item.tag === 'code' && item.attrs.language) el.dataset.language = item.attrs.language;
      (item.children || []).forEach(function (child) { el.appendChild(make(child)); });
      return el;
    }
    ast.forEach(function (item) { fragment.appendChild(make(item)); });
    return fragment;
  }

  function render(doc, markdown, openUrl) {
    var box = doc.createElement('div'); box.className = 'ni-text ni-markdown';
    box.appendChild(materialize(doc, parse(markdown), openUrl)); return box;
  }
  return { parse: parse, render: render, safeUrl: safeUrl, cleanSource: cleanSource };
}));
