// ============================================================================
// DevEco Switch · 局域网访问网页端
// 原生 JS，无构建链：fetch + EventSource，三视图 SPA。
// 视图：login（6 位令牌）→ list（项目/会话/搜索）→ chat（消息/审批/文件）
// ============================================================================

'use strict';

const $ = (id) => document.getElementById(id);
const TOKEN_KEY = 'dss_lan_token';

// ---------------------------------------------------------------------------
// 全局状态
// ---------------------------------------------------------------------------
const S = {
  token: localStorage.getItem(TOKEN_KEY) || '',
  readOnly: false,
  projects: [],
  projectId: '',
  convs: [],
  conv: null,
  messages: [],
  hasMore: false,
  streaming: null,
  tool: null,
  pendingMap: {},
  images: [],
  es: null,
  fileModalOpen: null,
  unread: 0,
  atBottom: true,
};

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------
function toast(msg, kind) {
  let t = $('toast');
  if (!t) {
    t = document.createElement('div');
    t.id = 'toast';
    document.body.appendChild(t);
  }
  t.textContent = msg;
  t.className = 'show' + (kind ? ' ' + kind : '');
  clearTimeout(t._timer);
  t._timer = setTimeout(() => { t.className = ''; }, kind === 'err' ? 3000 : 2200);
}

function pad(n) { return n < 10 ? '0' + n : '' + n; }

function fmtTime(ts) {
  if (!ts) return '';
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return Math.floor(diff / 60) + ' 分钟前';
  if (diff < 86400) return Math.floor(diff / 3600) + ' 小时前';
  if (diff < 7 * 86400) return Math.floor(diff / 86400) + ' 天前';
  const d = new Date(ts * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function escapeHtml(s) {
  if (s == null) return '';
  return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

async function copyText(text) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* ignore */ }
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.cssText = 'position:fixed;opacity:0;left:-9999px';
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try { ok = document.execCommand('copy'); } catch { ok = false; }
  document.body.removeChild(ta);
  return ok;
}

function highlight(text, q) {
  if (!q) return escapeHtml(text);
  const escText = escapeHtml(text);
  const escQ = q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return escText.replace(new RegExp(escQ, 'gi'), (m) => `<mark>${m}</mark>`);
}

// 简化 Markdown：代码块 + 行内代码 + 粗体 + 标题 + 列表 + 引用 + 链接
function md(text) {
  if (!text) return '';
  const codeBlocks = [];
  let s = text.replace(/```([a-zA-Z0-9_+\-#]*)\n?([\s\S]*?)```/g, (m, lang, code) => {
    const idx = codeBlocks.length;
    const safe = escapeHtml(code.replace(/\n$/, ''));
    codeBlocks.push(
      `<div class="md-code">`
        + `<div class="code-head">`
        + (lang ? `<span class="lang">${escapeHtml(lang)}</span>` : '<span></span>')
        + `<button class="copy-code" type="button" title="复制代码">复制</button>`
        + `</div>`
        + `<pre class="code-body" data-lang="${escapeHtml(lang)}">${highlightCode(safe, lang)}</pre>`
        + `</div>`
    );
    return `\u0000CODEBLOCK${idx}\u0000`;
  });

  s = escapeHtml(s);
  s = renderBlockMd(s);
  s = renderInlineMd(s);
  s = s.replace(/\u0000CODEBLOCK(\d+)\u0000/g, (_, i) => codeBlocks[Number(i)] || '');
  return s;
}

function renderBlockMd(s) {
  const lines = s.split('\n');
  const out = [];
  let listType = null;
  let quoteLines = [];

  const flushList = () => {
    if (listType) { out.push(`</${listType}>`); listType = null; }
  };
  const flushQuote = () => {
    if (quoteLines.length) {
      out.push(`<blockquote class="md-quote">${quoteLines.join('<br>')}</blockquote>`);
      quoteLines = [];
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const qm = line.match(/^&gt;\s?(.*)$/);
    if (qm) { flushList(); quoteLines.push(qm[1]); continue; }
    flushQuote();
    if (/^---+\s*$/.test(line)) { flushList(); out.push('<hr class="md-hr">'); continue; }
    const hm = line.match(/^(#{1,3})\s+(.+)$/);
    if (hm) {
      flushList();
      const level = hm[1].length;
      out.push(`<h${level} class="md-h md-h${level}">${hm[2]}</h${level}>`);
      continue;
    }
    const um = line.match(/^[\-\*]\s+(.+)$/);
    if (um) {
      if (listType !== 'ul') { flushList(); out.push('<ul class="md-list">'); listType = 'ul'; }
      out.push(`<li>${um[1]}</li>`);
      continue;
    }
    const om = line.match(/^\d+[.)]\s+(.+)$/);
    if (om) {
      if (listType !== 'ol') { flushList(); out.push('<ol class="md-list">'); listType = 'ol'; }
      out.push(`<li>${om[1]}</li>`);
      continue;
    }
    if (!line.trim()) { flushList(); out.push(''); continue; }
    flushList();
    out.push(line);
  }
  flushList();
  flushQuote();
  // 每个非空行 = 一个 <p>，段间自动 4px margin
  // 这样单换行的两行也分段（解决"对话挤一起"问题）
  const final = [];
  for (const ln of out) {
    if (ln === '') { final.push(''); continue; }  // 空行占位（保持节奏）
    if (/^<(h\d|ul|ol|hr|blockquote|p|div|table)/.test(ln)) {
      final.push(ln);
    } else {
      final.push(`<p class="md-p">${ln}</p>`);
    }
  }
  // 用空串拼接，避免 pre-wrap 模式下空行变成强制大段空白
  return final.join('');
}

function renderInlineMd(s) {
  return s
    .replace(/`([^`]+)`/g, '<span class="md-inline">$1</span>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<span class="md-bold">$1</span>')
    .replace(/(^|[^\*])\*([^*\n]+)\*(?!\*)/g, '$1<span class="md-italic">$2</span>')
    .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a class="md-link" href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
}

function highlightCode(src, lang) {
  const L = (lang || '').toLowerCase().split(/[\s,]+/)[0];
  const commentStyle = (() => {
    if (/^(py|rb|sh|bash|yaml|yml)$/.test(L)) return { line: '#', block: null };
    if (/^(sql)$/.test(L)) return { line: '--', block: ['/*', '*/'] };
    if (/^(html|xml|vue)$/.test(L)) return { line: null, block: ['<!--', '-->'] };
    if (/^(css|scss|less)$/.test(L)) return { line: null, block: ['/*', '*/'] };
    if (/^(lua)$/.test(L)) return { line: '--', block: ['--[[', ']]'] };
    return { line: '//', block: ['/*', '*/'] };
  })();

  const tokens = [];
  const protect = (cls, text) => {
    const i = tokens.length;
    tokens.push(`<span class="tk-${cls}">${text}</span>`);
    return `\u0001${i}\u0001`;
  };

  let s = src;
  if (commentStyle.block) {
    const [bs, be] = commentStyle.block;
    const esc = be.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    s = s.replace(new RegExp(`${bs}[\\s\\S]*?${esc}`, 'g'), (m) => protect('cmt', m));
  }
  if (commentStyle.line) {
    const esc = commentStyle.line.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    s = s.replace(new RegExp(`${esc}.*$`, 'gm'), (m) => protect('cmt', m));
  }
  s = s.replace(/"([^"\\\n]|\\.)*"/g, (m) => protect('str', m))
       .replace(/'([^'\\\n]|\\.)*'/g, (m) => protect('str', m))
       .replace(/`([^`\\]|\\.)*`/g, (m) => protect('str', m));
  s = s.replace(/\b\d+(?:\.\d+)?\b/g, (m) => protect('num', m));
  const kws = KEYWORDS[L] || KEYWORDS._;
  if (kws) {
    const re = new RegExp(`\\b(${kws.join('|')})\\b`, 'g');
    s = s.replace(re, (m) => protect('kw', m));
  }
  s = s.replace(/\b([a-zA-Z_$][\w$]*)\s*(?=\()/g, (m, name) => {
    if (KEYWORDS_ALL.has(name)) return m;
    return protect('fn', name);
  });
  s = s.replace(/\u0001(\d+)\u0001/g, (_, i) => tokens[Number(i)] || '');
  return s;
}

const KEYWORDS_ALL = new Set();
const KEYWORDS = {
  _: ['if', 'else', 'for', 'while', 'return', 'in', 'of', 'true', 'false', 'null', 'nil', 'yes', 'no'],
  js: ['const', 'let', 'var', 'function', 'return', 'if', 'else', 'for', 'while', 'do', 'switch', 'case', 'break', 'continue', 'class', 'extends', 'new', 'this', 'super', 'import', 'export', 'from', 'as', 'default', 'async', 'await', 'try', 'catch', 'finally', 'throw', 'typeof', 'instanceof', 'in', 'of', 'true', 'false', 'null', 'undefined'],
  ts: ['const', 'let', 'var', 'function', 'return', 'if', 'else', 'for', 'while', 'do', 'switch', 'case', 'break', 'continue', 'class', 'extends', 'implements', 'interface', 'type', 'enum', 'new', 'this', 'super', 'import', 'export', 'from', 'as', 'default', 'async', 'await', 'try', 'catch', 'finally', 'throw', 'typeof', 'instanceof', 'in', 'of', 'public', 'private', 'protected', 'readonly', 'true', 'false', 'null', 'undefined'],
  py: ['def', 'class', 'return', 'if', 'elif', 'else', 'for', 'while', 'break', 'continue', 'import', 'from', 'as', 'try', 'except', 'finally', 'raise', 'with', 'lambda', 'pass', 'yield', 'True', 'False', 'None', 'and', 'or', 'not', 'in', 'is'],
  java: ['public', 'private', 'protected', 'class', 'interface', 'extends', 'implements', 'static', 'final', 'abstract', 'void', 'return', 'if', 'else', 'for', 'while', 'do', 'switch', 'case', 'break', 'continue', 'new', 'this', 'super', 'try', 'catch', 'finally', 'throw', 'throws', 'import', 'package', 'true', 'false', 'null'],
  go: ['func', 'var', 'const', 'type', 'struct', 'interface', 'return', 'if', 'else', 'for', 'range', 'switch', 'case', 'default', 'break', 'continue', 'go', 'defer', 'package', 'import', 'map', 'chan', 'select', 'true', 'false', 'nil'],
  rs: ['fn', 'let', 'mut', 'const', 'static', 'struct', 'enum', 'trait', 'impl', 'pub', 'use', 'mod', 'return', 'if', 'else', 'for', 'while', 'loop', 'match', 'break', 'continue', 'in', 'as', 'where', 'true', 'false', 'self', 'Self'],
  rb: ['def', 'class', 'module', 'return', 'if', 'elsif', 'else', 'unless', 'do', 'while', 'until', 'for', 'in', 'break', 'next', 'begin', 'rescue', 'ensure', 'raise', 'yield', 'lambda', 'proc', 'require', 'true', 'false', 'nil', 'and', 'or', 'not'],
  sh: ['if', 'then', 'else', 'elif', 'fi', 'for', 'while', 'do', 'done', 'case', 'esac', 'in', 'function', 'return', 'export', 'local', 'echo', 'cd', 'pwd', 'true', 'false'],
  sql: ['select', 'from', 'where', 'and', 'or', 'not', 'in', 'as', 'join', 'left', 'right', 'inner', 'outer', 'on', 'group', 'by', 'order', 'having', 'limit', 'offset', 'insert', 'into', 'values', 'update', 'set', 'delete', 'create', 'table', 'drop', 'alter', 'add', 'column', 'index', 'primary', 'key', 'foreign', 'references', 'null', 'true', 'false'],
  css: ['important'],
};
Object.values(KEYWORDS).forEach((arr) => arr.forEach((k) => KEYWORDS_ALL.add(k)));

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------
async function api(path, opts = {}) {
  const headers = { Authorization: `Bearer ${S.token}`, ...(opts.headers || {}) };
  if (opts.body !== undefined) headers['Content-Type'] = 'application/json';
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 10000);
  let res;
  try {
    res = await fetch(path, { ...opts, headers, signal: ctrl.signal });
  } catch (e) {
    if (e.name === 'AbortError') throw new Error('请求超时（10s）');
    throw e;
  } finally {
    clearTimeout(timer);
  }
  if (res.status === 401) {
    let retry = '';
    try {
      const j = await res.json();
      if (j.error && j.error.retry_after) retry = `（锁定 ${Math.ceil(j.error.retry_after)}s）`;
    } catch { /* ignore */ }
    logout(`令牌无效或已锁定${retry}`);
    throw new Error('unauthorized');
  }
  if (!res.ok) {
    let msg = `HTTP ${res.status}`;
    try {
      const j = await res.json();
      if (j.error && j.error.message) msg = j.error.message;
    } catch { /* ignore */ }
    throw new Error(msg);
  }
  return res.json();
}

// ---------------------------------------------------------------------------
// 浏览器通知
// ---------------------------------------------------------------------------
let notifyEnabled = localStorage.getItem('dss_notify') === '1';
function notifySupported() { return typeof Notification !== 'undefined'; }
async function requestNotifyPermission() {
  if (!notifySupported() || Notification.permission === 'granted') return;
  try {
    const p = await Notification.requestPermission();
    if (p === 'granted') { notifyEnabled = true; localStorage.setItem('dss_notify', '1'); }
  } catch { /* ignore */ }
}
function maybeNotify(convId, title, body) {
  if (!notifyEnabled || !notifySupported() || Notification.permission !== 'granted') return;
  if (document.hasFocus() && S.conv && S.conv.id === convId) return;
  try {
    const n = new Notification(title, { body, tag: 'dss-' + convId, icon: '/icon.svg' });
    n.onclick = () => {
      n.close();
      window.focus();
      const conv = S.convs.find((c) => c.id === convId);
      if (conv) openConv(conv);
    };
    setTimeout(() => n.close(), 10000);
  } catch { /* ignore */ }
}

// ---------------------------------------------------------------------------
// 语音输入
// ---------------------------------------------------------------------------
const SpeechRec = window.SpeechRecognition || window.webkitSpeechRecognition;
let rec = null;
function initMic() {
  if (!SpeechRec) {
    const btn = $('micBtn');
    if (btn) btn.hidden = true;
    return;
  }
  $('micBtn').addEventListener('click', toggleMic);
}
function toggleMic() {
  if (rec) { rec.stop(); return; }
  try {
    rec = new SpeechRec();
    rec.lang = navigator.language || 'zh-CN';
    rec.interimResults = false;
    rec.maxAlternatives = 1;
    rec.continuous = false;
    rec.onresult = (e) => {
      const text = Array.from(e.results).map((r) => r[0].transcript).join('');
      const input = $('input');
      input.value = (input.value ? input.value + ' ' : '') + text;
      input.dispatchEvent(new Event('input'));
      input.focus();
    };
    rec.onend = () => { rec = null; $('micBtn').classList.remove('recording'); };
    rec.onerror = (e) => {
      rec = null;
      $('micBtn').classList.remove('recording');
      if (e.error !== 'aborted' && e.error !== 'no-speech') toast('语音识别失败：' + e.error, 'err');
    };
    rec.start();
    $('micBtn').classList.add('recording');
  } catch { toast('语音输入不可用', 'err'); }
}

// ---------------------------------------------------------------------------
// 图片
// ---------------------------------------------------------------------------
function initImagePicker() {
  $('imgBtn').addEventListener('click', () => $('imgInput').click());
  $('imgInput').addEventListener('change', (e) => {
    const files = [...(e.target.files || [])];
    e.target.value = '';
    const remaining = 4 - S.images.length;
    if (files.length > remaining) {
      toast(`最多同时发送 4 张图片，已截取前 ${remaining} 张`, 'err');
    }
    files.slice(0, remaining).forEach((f) => {
      if (!f.type.startsWith('image/')) return;
      const reader = new FileReader();
      reader.onload = () => {
        S.images.push(String(reader.result));
        renderImagePreview();
      };
      reader.readAsDataURL(f);
    });
  });
}
function renderImagePreview() {
  const wrap = $('imgPreview');
  wrap.hidden = S.images.length === 0;
  wrap.innerHTML = '';
  S.images.forEach((url, idx) => {
    const t = document.createElement('div');
    t.className = 'thumb';
    t.innerHTML = `<img src="${url}" alt=""><span class="rm" title="移除">×</span>`;
    t.querySelector('.rm').addEventListener('click', () => {
      S.images.splice(idx, 1);
      renderImagePreview();
    });
    wrap.appendChild(t);
  });
}

// ---------------------------------------------------------------------------
// 视图切换
// ---------------------------------------------------------------------------
const VIEWS = { login: 'view-login', list: 'view-list', chat: 'view-chat' };
function showView(name) {
  Object.entries(VIEWS).forEach(([k, id]) => { $(id).hidden = k !== name; });
}

// ---------------------------------------------------------------------------
// 登录
// ---------------------------------------------------------------------------
function buildTokenGrid() {
  const grid = $('tokenGrid');
  grid.innerHTML = '';
  for (let i = 0; i < 6; i++) {
    const input = document.createElement('input');
    input.className = 'token-cell';
    input.type = 'text';
    input.inputMode = 'numeric';
    input.maxLength = 1;
    input.autocomplete = 'one-time-code';
    input.dataset.idx = String(i);
    grid.appendChild(input);
  }
  const cells = [...grid.children];
  grid.addEventListener('input', (e) => {
    const el = e.target;
    if (!el.classList.contains('token-cell')) return;
    const v = el.value.replace(/\D/g, '');
    if (v.length > 1) {
      const digits = v.slice(0, 6).split('');
      digits.forEach((d, idx) => { cells[idx].value = d; cells[idx].classList.add('done'); });
      const last = cells[Math.min(digits.length, 5)];
      if (digits.length === 6) { last.blur(); tryLogin(); } else last.focus();
      return;
    }
    el.value = v;
    el.classList.toggle('done', v !== '');
    if (v) {
      const next = cells[Number(el.dataset.idx) + 1];
      if (next) next.focus();
      else { el.blur(); tryLogin(); }
    }
  });
  grid.addEventListener('keydown', (e) => {
    if (e.key === 'Backspace' && e.target.value === '') {
      const idx = Number(e.target.dataset.idx);
      if (idx > 0) cells[idx - 1].focus();
    } else if (e.key === 'ArrowLeft') {
      const idx = Number(e.target.dataset.idx);
      if (idx > 0) cells[idx - 1].focus();
    } else if (e.key === 'ArrowRight') {
      const idx = Number(e.target.dataset.idx);
      if (idx < 5) cells[idx + 1].focus();
    }
  });
  const m = location.hash.match(/#(\d{6})/);
  if (m) {
    cells.forEach((c, i) => { c.value = m[1][i]; c.classList.add('done'); });
    location.hash = '';
  }
}
function getTokenInput() { return [...$('tokenGrid').children].map((c) => c.value).join(''); }

async function tryLogin() {
  const token = getTokenInput();
  if (token.length !== 6) return;
  S.token = token;
  const btn = $('loginBtn');
  btn.disabled = true;
  const hint = $('loginHint');
  hint.className = 'hint';
  hint.textContent = '正在连接…';
  try {
    await api('/api/projects');
    localStorage.setItem(TOKEN_KEY, token);
    hint.textContent = '';
    hint.className = 'hint ok';
    connectSSE();
    requestNotifyPermission();
    await enterList();
  } catch (e) {
    if (e.message !== 'unauthorized') {
      hint.className = 'hint err';
      hint.textContent = '连接失败：' + e.message;
    }
  } finally {
    btn.disabled = false;
  }
}

function logout(msg) {
  localStorage.removeItem(TOKEN_KEY);
  S.token = '';
  if (S.es) { S.es.close(); S.es = null; }
  S.streaming = null;
  S.tool = null;
  S.conv = null;
  S.messages = [];
  S.pendingMap = {};
  S.images = [];
  S.unread = 0;
  showView('login');
  const dot = $('connDot');
  if (dot) dot.classList.remove('on', 'off');
  const hint = $('loginHint');
  hint.className = 'hint err';
  hint.textContent = msg || '';
  buildTokenGrid();
}

// ---------------------------------------------------------------------------
// 项目切换抽屉
// ---------------------------------------------------------------------------
async function loadProjects() {
  const projects = await api('/api/projects');
  S.projects = projects;
  if (!S.projectId) S.projectId = projects[0] && projects[0].id || '';
  updateProjectLabel();
}
function updateProjectLabel() {
  const p = S.projects.find((x) => x.id === S.projectId);
  const label = $('projectSelectLabel');
  if (label) label.textContent = p ? p.name : '选择项目';
}
function openProjectDrawer() {
  const drawer = $('projectDrawer');
  if (!drawer) return;
  renderProjectList('');
  drawer.hidden = false;
  setTimeout(() => { const s = $('projectSearchInput'); if (s) { s.value = ''; s.focus(); } }, 250);
  $('projectSelect').classList.add('open');
}
function closeProjectDrawer() {
  $('projectDrawer').hidden = true;
  $('projectSelect').classList.remove('open');
}
function renderProjectList(q) {
  const list = $('projectDrawerList');
  if (!list) return;
  const ql = (q || '').toLowerCase().trim();
  const filtered = ql
    ? S.projects.filter((p) => (p.name + ' ' + (p.path || '')).toLowerCase().includes(ql))
    : S.projects.slice();
  if (!filtered.length) {
    list.innerHTML = '<div class="drawer-empty">没有匹配的项目</div>';
    return;
  }
  filtered.sort((a, b) => {
    if (a.id === 'global') return -1;
    if (b.id === 'global') return 1;
    return (a.name || '').localeCompare(b.name || '');
  });
  list.innerHTML = filtered.map((p) => {
    const isActive = p.id === S.projectId;
    const isGlobal = p.id === 'global';
    const initial = (p.name || '?').charAt(0).toUpperCase();
    return `
      <div class="drawer-item ${isActive ? 'active' : ''} ${isGlobal ? 'global' : ''}" data-pid="${escapeHtml(p.id)}">
        <div class="di-avatar">${escapeHtml(initial)}</div>
        <div class="di-body">
          <div class="di-name">
            <span>${escapeHtml(p.name || p.id)}</span>
            <svg class="check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </div>
          ${!isGlobal && p.path ? `<div class="di-path">${escapeHtml(p.path)}</div>` : ''}
          <div class="di-meta">
            ${p.last_active_at ? `<span>${escapeHtml(fmtTime(p.last_active_at))}</span>` : '<span class="di-empty">无活动</span>'}
          </div>
        </div>
      </div>`;
  }).join('');
  list.querySelectorAll('.drawer-item').forEach((el) => {
    el.addEventListener('click', () => switchProject(el.dataset.pid));
  });
}
async function switchProject(pid) {
  if (!pid || pid === S.projectId) { closeProjectDrawer(); return; }
  S.projectId = pid;
  $('projectSelect').value = pid;
  updateProjectLabel();
  $('searchInput').value = '';
  $('searchResults').hidden = true;
  $('convList').hidden = false;
  closeProjectDrawer();
  await loadConvs(pid);
}

// ---------------------------------------------------------------------------
// 会话列表
// ---------------------------------------------------------------------------
async function enterList() {
  showView('list');
  try {
    const status = await api('/api/lan/status');
    S.readOnly = !!status.read_only;
  } catch { /* ignore */ }
  await loadProjects();
  await loadConvs(S.projectId);
}

async function loadConvs(projectId) {
  const listEl = $('convList');
  const skel = $('listSkeleton');
  if (!projectId) { listEl.innerHTML = ''; skel.hidden = true; $('listEmpty').hidden = false; return; }
  $('listEmpty').hidden = true;
  $('searchResults').hidden = true;
  if (!S.convs.length) skel.hidden = false;
  try {
    const [convs, pending] = await Promise.all([
      api(`/api/projects/${encodeURIComponent(projectId)}/conversations`),
      api(`/api/projects/${encodeURIComponent(projectId)}/pending`).catch(() => []),
    ]);
    S.convs = convs;
    mergePending(pending);
    skel.hidden = true;
    renderConvList();
  } catch (e) {
    skel.hidden = true;
    listEl.innerHTML = '';
    $('listEmpty').hidden = false;
    $('listEmpty').innerHTML = `<svg class="empty-ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>加载失败：${escapeHtml(e.message)}`;
  }
}

function mergePending(list) {
  S.pendingMap = {};
  (list || []).forEach((p) => {
    (S.pendingMap[p.conversation_id] = S.pendingMap[p.conversation_id] || []).push(p);
  });
}

async function refreshPending() {
  if (!S.projectId) return;
  try {
    const list = await api(`/api/projects/${encodeURIComponent(S.projectId)}/pending`).catch(() => []);
    mergePending(list);
    if (S.conv) renderPendingCards(); else renderConvList();
  } catch { /* ignore */ }
}

function renderConvList() {
  const list = $('convList');
  list.innerHTML = '';
  if (!S.convs.length) { $('listEmpty').hidden = false; return; }
  $('listEmpty').hidden = true;
  S.convs.forEach((c) => {
    const li = document.createElement('li');
    let cls = 'conv-item';
    if (c.is_pinned) cls += ' pinned';
    if (c.archived) cls += ' archived';
    li.className = cls;
    const badge = (S.pendingMap[c.id] || []).length;
    const meta = [];
    meta.push(fmtTime(c.updated_at));
    if (c.is_pinned) meta.push('<span class="tag">置顶</span>');
    if (c.archived) meta.push('<span class="tag">已归档</span>');
    li.innerHTML = `
      <div class="body">
        <div class="name">${escapeHtml(c.title || '（无标题）')}</div>
        <div class="meta">${meta.join(' · ')}</div>
      </div>
      ${!S.readOnly ? `
      <div class="conv-actions">
        <button data-act="pin" title="${c.is_pinned ? '取消置顶' : '置顶'}" aria-label="置顶">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="17" x2="12" y2="22"/><path d="M5 17h14l-1.7-1.7a2 2 0 0 1 0-2.8L18 11H6l1.7 1.5a2 2 0 0 1 0 2.8L5 17z"/></svg>
        </button>
        <button data-act="archive" title="${c.archived ? '取消归档' : '归档'}" aria-label="归档">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="21 8 21 21 3 21 3 8"/><rect x="1" y="3" width="22" height="5"/><line x1="10" y1="12" x2="14" y2="12"/></svg>
        </button>
      </div>` : ''}
      ${badge ? `<span class="badge">${badge}</span>` : ''}`;
    li.addEventListener('click', (e) => {
      const actBtn = e.target.closest('[data-act]');
      if (actBtn) {
        e.stopPropagation();
        quickAction(c, actBtn.dataset.act);
        return;
      }
      openConv(c);
    });
    // 长按弹上下文菜单（移动端）
    let pressTimer = null;
    li.addEventListener('touchstart', (e) => {
      if (e.touches.length !== 1) return;
      pressTimer = setTimeout(() => {
        showContextMenu(e.touches[0].clientX, e.touches[0].clientY, c);
        if (navigator.vibrate) navigator.vibrate(10);
      }, 500);
    }, { passive: true });
    li.addEventListener('touchend', () => clearTimeout(pressTimer));
    li.addEventListener('touchmove', () => clearTimeout(pressTimer));
    list.appendChild(li);
  });
}

function showContextMenu(x, y, conv) {
  const m = $('contextMenu');
  if (!m) return;
  m.innerHTML = `
    <button data-act="rename">重命名</button>
    <button data-act="pin">${conv.is_pinned ? '取消置顶' : '置顶'}</button>
    <button data-act="archive">${conv.archived ? '取消归档' : '归档'}</button>
    <hr>
    <button data-act="delete" class="danger-text">删除</button>
  `;
  m.hidden = false;
  // 定位：默认在长按位置，超出屏幕则贴边
  m.style.left = Math.min(x, window.innerWidth - 200) + 'px';
  m.style.top = Math.min(y, window.innerHeight - 200) + 'px';
  m.style.position = 'fixed';
  m.onclick = (e) => {
    const btn = e.target.closest('[data-act]');
    if (!btn) return;
    m.hidden = true;
    if (btn.dataset.act === 'delete') {
      dlg({ title: '删除会话', message: '确定删除「' + (conv.title || '（无标题）') + '」吗？此操作不可恢复！', okText: '删除', danger: true, cancelText: '取消' })
        .then((ok) => ok && api(`/api/conversations/${encodeURIComponent(conv.id)}/delete`, { method: 'POST', body: '{}' })
          .then(() => loadConvs(S.projectId)).then(() => toast('已删除', 'ok')).catch((e) => toast('删除失败：' + e.message, 'err')));
    } else if (btn.dataset.act === 'rename') {
      dlg({ title: '重命名会话', message: '输入新标题：', withInput: true, inputValue: conv.title || '', inputPlaceholder: '会话标题', okText: '保存' })
        .then((t) => {
          if (t === null || t === undefined) return;
          const v = String(t).trim();
          if (!v) return toast('标题不能为空', 'err');
          api(`/api/conversations/${encodeURIComponent(conv.id)}/rename`, { method: 'POST', body: JSON.stringify({ title: v }) })
            .then(() => { conv.title = v; renderConvList(); toast('已重命名', 'ok'); });
        });
    } else {
      quickAction(conv, btn.dataset.act);
    }
  };
}

async function quickAction(conv, act) {
  if (S.readOnly) return;
  const cid = encodeURIComponent(conv.id);
  try {
    if (act === 'pin') {
      const r = await api(`/api/conversations/${cid}/pin`, { method: 'POST', body: JSON.stringify({ pinned: !conv.is_pinned }) });
      conv.is_pinned = !!r.is_pinned;
      toast(conv.is_pinned ? '已置顶' : '已取消置顶', 'ok');
    } else if (act === 'archive') {
      const r = await api(`/api/conversations/${cid}/archive`, { method: 'POST', body: JSON.stringify({ archived: !conv.archived }) });
      conv.archived = !!r.archived;
      toast(conv.archived ? '已归档' : '已取消归档', 'ok');
    }
    renderConvList();
  } catch (e) { toast('操作失败：' + e.message, 'err'); }
}

// ---------------------------------------------------------------------------
// 新建会话引导弹层
// ---------------------------------------------------------------------------
async function openNewConvDialog() {
  if (!S.projectId) { toast('请先选择项目', 'err'); return; }
  let info = null;
  try {
    info = await api(`/api/projects/${encodeURIComponent(S.projectId)}`);
  } catch { /* ignore */ }
  const agents = (info && info.agents) || ['default'];
  const models = (info && info.models) || [];
  const agentGroup = $('agentChips');
  const modelGroup = $('modelChips');
  if (agentGroup) {
    agentGroup.innerHTML = agents.map((a, i) =>
      `<button type="button" class="chip ${i === 0 ? 'selected' : ''}" data-val="${escapeHtml(a)}">${escapeHtml(a)}</button>`
    ).join('');
    agentGroup.querySelectorAll('.chip').forEach((b) => {
      b.addEventListener('click', () => {
        agentGroup.querySelectorAll('.chip').forEach((x) => x.classList.remove('selected'));
        b.classList.add('selected');
      });
    });
  }
  if (modelGroup) {
    modelGroup.innerHTML =
      `<button type="button" class="chip selected" data-val="">默认</button>` +
      models.map((m) => `<button type="button" class="chip" data-val="${escapeHtml(m)}">${escapeHtml(m)}</button>`).join('');
    modelGroup.querySelectorAll('.chip').forEach((b) => {
      b.addEventListener('click', () => {
        modelGroup.querySelectorAll('.chip').forEach((x) => x.classList.remove('selected'));
        b.classList.add('selected');
      });
    });
  }
  const ta = $('newConvFirstMsg'); if (ta) ta.value = '';
  const hint = $('newConvHint'); if (hint) { hint.textContent = ''; hint.className = 'form-hint'; }
  $('newConvModal').hidden = false;
}
function closeNewConvDialog() { $('newConvModal').hidden = true; }
async function submitNewConv() {
  if (!S.projectId) return;
  const selectedAgent = $('agentChips').querySelector('.chip.selected');
  const selectedModel = $('modelChips').querySelector('.chip.selected');
  const firstMsg = ($('newConvFirstMsg') || {}).value || '';
  const body = {};
  if (selectedAgent) body.agent = selectedAgent.dataset.val;
  if (selectedModel && selectedModel.dataset.val) body.model = selectedModel.dataset.val;
  if (firstMsg.trim()) body.initial_message = firstMsg.trim();
  const submit = $('newConvSubmit');
  submit.disabled = true;
  try {
    const conv = await api(`/api/projects/${encodeURIComponent(S.projectId)}/conversations`, {
      method: 'POST', body: JSON.stringify(body),
    });
    closeNewConvDialog();
    await loadConvs(S.projectId);
    openConv(conv);
    if (firstMsg.trim()) {
      $('input').value = firstMsg.trim();
      sendMessage();
    }
  } catch (e) {
    const hint = $('newConvHint');
    if (hint) { hint.textContent = '创建失败：' + e.message; hint.className = 'form-hint err'; }
  } finally {
    submit.disabled = false;
  }
}

// ---------------------------------------------------------------------------
// 自定义对话框（替换浏览器原生 confirm / alert / prompt）
// ---------------------------------------------------------------------------
function dlg({ title = '提示', message = '', okText = '确定', cancelText = '取消', danger = false, withInput = false, inputValue = '', inputPlaceholder = '' } = {}) {
  return new Promise((resolve) => {
    const modal = $('customDialog');
    $('dlgTitle').textContent = title;
    $('dlgMsg').textContent = message;
    const inp = $('dlgInput');
    if (withInput) {
      inp.hidden = false;
      inp.value = inputValue;
      inp.placeholder = inputPlaceholder;
      setTimeout(() => inp.focus(), 50);
    } else {
      inp.hidden = true;
    }
    const okBtn = $('dlgOk');
    okBtn.textContent = okText;
    okBtn.className = 'btn' + (danger ? ' danger' : ' primary');
    $('dlgCancel').textContent = cancelText;
    $('dlgCancel').hidden = !cancelText;
    modal.hidden = false;
    const cleanup = (val) => { modal.hidden = true; okBtn.onclick = null; $('dlgCancel').onclick = null; resolve(val); };
    okBtn.onclick = () => cleanup(withInput ? inp.value : true);
    $('dlgCancel').onclick = () => cleanup(withInput ? null : false);
    inp.onkeydown = (e) => {
      if (e.key === 'Enter') { e.preventDefault(); cleanup(inp.value); }
      if (e.key === 'Escape') { e.preventDefault(); cleanup(null); }
    };
  });
}

async function newConversation() {
  await openNewConvDialog();
}

// ---------------------------------------------------------------------------
// 会话页
// ---------------------------------------------------------------------------
async function openConv(conv) {
  S.conv = conv;
  S.messages = [];
  S.hasMore = false;
  S.streaming = null;
  S.tool = null;
  S.unread = 0;
  S.atBottom = true;
  $('chatTitle').textContent = conv.title || '（无标题）';
  $('msgList').innerHTML = '';
  $('toolBar').hidden = true;
  $('scrollBottom').classList.remove('show');
  renderComposer();
  syncMenuLabels();
  showView('chat');
  await reloadMessages();
  renderPendingCards();
  connectSSE();
}

function renderComposer() {
  $('composer').hidden = !!S.readOnly;
  $('sendBtn').hidden = S.readOnly;
  $('stopBtn').hidden = true;
}
function syncMenuLabels() {
  if (!S.conv) return;
  const pinLabel = $('pinLabel');
  const arcLabel = $('archiveLabel');
  if (pinLabel) pinLabel.textContent = S.conv.is_pinned ? '取消置顶' : '置顶';
  if (arcLabel) arcLabel.textContent = S.conv.archived ? '取消归档' : '归档';
}

async function reloadMessages() {
  if (!S.conv) return;
  try {
    const page = await api(`/api/conversations/${encodeURIComponent(S.conv.id)}/messages?limit=60`);
    S.messages = page.messages || [];
    S.hasMore = !!page.hasMore;
    renderMessages(true);
  } catch (e) {
    toast('加载消息失败：' + e.message, 'err');
  }
}

let loadingOlder = false;
async function loadOlder() {
  if (!S.conv || !S.hasMore || !S.messages.length || loadingOlder) return;
  loadingOlder = true;
  try {
    const beforeId = S.messages[0].id;
    const page = await api(
      `/api/conversations/${encodeURIComponent(S.conv.id)}/messages?limit=60&before=${encodeURIComponent(beforeId)}`
    );
    if (!page.messages || !page.messages.length) { S.hasMore = false; renderMessages(false); return; }
    S.messages = [...page.messages, ...S.messages];
    S.hasMore = !!page.hasMore;
    const list = $('msgList');
    const oldHeight = list.scrollHeight;
    renderMessages(false);
    list.scrollTop = list.scrollHeight - oldHeight;
  } catch (e) {
    toast('加载更早消息失败：' + e.message, 'err');
  } finally {
    loadingOlder = false;
  }
}

function renderMessages(scrollToBottom) {
  const list = $('msgList');
  const wasAtBottom = scrollToBottom !== undefined ? scrollToBottom : S.atBottom;
  list.innerHTML = '';
  if (S.hasMore) {
    const div = document.createElement('div');
    div.className = 'load-more';
    div.innerHTML = '<button>加载更早消息</button>';
    div.querySelector('button').addEventListener('click', loadOlder);
    list.appendChild(div);
  }
  S.messages.forEach((m) => list.appendChild(messageEl(m)));
  if (S.streaming && S.streaming.el) list.appendChild(S.streaming.el);
  if (wasAtBottom) { list.scrollTop = list.scrollHeight; S.unread = 0; updateScrollBottom(); }
}

function updateScrollBottom() {
  const list = $('msgList');
  const dist = list.scrollHeight - list.scrollTop - list.clientHeight;
  S.atBottom = dist < 80;
  const btn = $('scrollBottom');
  if (S.atBottom) {
    btn.classList.remove('show');
    S.unread = 0;
    const u = btn.querySelector('.unread');
    if (u) u.hidden = true;
  } else {
    btn.classList.add('show');
    const u = btn.querySelector('.unread');
    if (u && S.unread > 0) { u.hidden = false; u.textContent = S.unread > 99 ? '99+' : String(S.unread); }
    else if (u) u.hidden = true;
  }
}

function messageEl(m) {
  const wrap = document.createElement('div');
  const isUser = m.role === 'user';
  wrap.className = 'msg ' + (isUser ? 'user' : 'assistant');
  wrap.dataset.mid = m.id;
  let filesHtml = '';
  let modified = [];
  try { modified = m.modified_files_json ? JSON.parse(m.modified_files_json) : []; } catch { /* ignore */ }
  if (modified && modified.length) {
    filesHtml = `<div class="files">${modified
      .map((f) => `<span class="file-chip" data-path="${escapeHtml(f)}" title="点击查看 ${escapeHtml(f)}">${escapeHtml(f)}</span>`)
      .join('')}</div>`;
  }
  const roleLabel = isUser ? '我' : 'AI';
  const avatar = isUser
    ? ''
    : `<div class="avatar" aria-hidden="true">
         <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
           <rect x="3" y="6" width="18" height="14" rx="3"/>
           <path d="M8 2v4M16 2v4M3 11h18"/>
           <circle cx="9" cy="14" r="1.2" fill="currentColor"/>
           <circle cx="15" cy="14" r="1.2" fill="currentColor"/>
           <path d="M9.5 17h5"/>
         </svg>
       </div>`;
  wrap.innerHTML = `
    ${avatar}
    <div class="bubble">
      <div class="msg-actions" role="toolbar" aria-label="消息操作">
        <button class="copy-msg" title="复制消息" aria-label="复制消息">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
        </button>
      </div>
      ${md(m.content || '')}${filesHtml}
      <div class="mtime">${fmtTime(m.created_at)}${m.model ? ' · ' + escapeHtml(m.model) : ''} · ${roleLabel}</div>
    </div>`;
  const copyBtn = wrap.querySelector('.copy-msg');
  const doCopy = async (btn) => {
    const ok = await copyText(m.content || '');
    btn.classList.toggle('copied', ok);
    const orig = btn.innerHTML;
    btn.innerHTML = ok
      ? '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>'
      : '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>';
    setTimeout(() => { btn.innerHTML = orig; btn.classList.remove('copied'); }, 1200);
  };
  copyBtn.addEventListener('click', (e) => { e.stopPropagation(); doCopy(copyBtn); });
  const bubble = wrap.querySelector('.bubble');
  let pressTimer = null;
  bubble.addEventListener('touchstart', (e) => {
    if (e.target.closest('button, a, .file-chip, .md-code, input, textarea')) return;
    pressTimer = setTimeout(() => {
      doCopy(copyBtn);
      if (navigator.vibrate) navigator.vibrate(15);
    }, 500);
  }, { passive: true });
  bubble.addEventListener('touchend', () => clearTimeout(pressTimer));
  bubble.addEventListener('touchmove', () => clearTimeout(pressTimer));
  bubble.addEventListener('touchcancel', () => clearTimeout(pressTimer));
  bubble.addEventListener('contextmenu', (e) => {
    if (e.target.closest('button, a, .file-chip, .md-code')) return;
    e.preventDefault();
    doCopy(copyBtn);
  });
  wrap.querySelectorAll('.file-chip').forEach((chip) => {
    chip.addEventListener('click', (e) => { e.stopPropagation(); openFile(chip.dataset.path); });
  });
  wrap.querySelectorAll('.copy-code').forEach((btn) => {
    btn.addEventListener('click', async (e) => {
      e.stopPropagation();
      const pre = btn.closest('.md-code').querySelector('pre');
      const ok = await copyText(pre ? pre.textContent : '');
      btn.classList.toggle('copied', ok);
      btn.textContent = ok ? '已复制' : '失败';
      setTimeout(() => { btn.classList.remove('copied'); btn.textContent = '复制'; }, 1500);
    });
  });
  return wrap;
}

// ---------------------------------------------------------------------------
// 流式 + 工具条
// ---------------------------------------------------------------------------
function ensureStreamingPlaceholder(convId) {
  if (S.streaming && S.streaming.convId !== convId) return null;
  if (!S.streaming) S.streaming = { convId, text: '', el: null };
  if (!S.streaming.el) {
    const el = document.createElement('div');
    el.className = 'msg assistant streaming';
    el.innerHTML = `
      <div class="avatar" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="6" width="18" height="14" rx="3"/>
          <path d="M8 2v4M16 2v4M3 11h18"/>
          <circle cx="9" cy="14" r="1.2" fill="currentColor"/>
          <circle cx="15" cy="14" r="1.2" fill="currentColor"/>
          <path d="M9.5 17h5"/>
        </svg>
      </div>
      <div class="bubble"><div class="msg-actions" hidden></div><span class="stream-text"></span></div>`;
    $('msgList').appendChild(el);
    S.streaming.el = el;
    if (S.atBottom) $('msgList').scrollTop = $('msgList').scrollHeight;
  }
  return S.streaming.el;
}

function appendStream(convId, delta) {
  if (!S.conv || convId !== S.conv.id) return;
  const el = ensureStreamingPlaceholder(convId);
  if (!el) return;
  S.streaming.text += delta;
  const text = el.querySelector('.stream-text');
  if (text) text.textContent = S.streaming.text;
  const list = $('msgList');
  if (list.scrollHeight - list.scrollTop - list.clientHeight < 160) {
    list.scrollTop = list.scrollHeight;
    S.atBottom = true;
  } else {
    S.atBottom = false;
    S.unread++;
    updateScrollBottom();
  }
}

function endStreaming() {
  S.streaming = null;
  S.tool = null;
  $('toolBar').hidden = true;
  $('stopBtn').hidden = true;
  $('sendBtn').hidden = S.readOnly;
}

function renderTool(convId, tool) {
  if (!S.conv || convId !== S.conv.id) return;
  const bar = $('toolBar');
  const levelLabel = tool.level === 'L2' ? '危险' : tool.level === 'L1' ? '写入' : '只读';
  const pct = tool.total ? Math.min(100, Math.round((tool.round / tool.total) * 100)) : null;
  bar.innerHTML = `
    <div class="row">
      <span class="name">${escapeHtml(tool.tool)}</span>
      ${pct != null ? `<span class="round">${tool.round}/${tool.total}</span>` : ''}
      <span class="bar"><i style="width:${pct != null ? pct : 100}%"></i></span>
      <span class="state">${escapeHtml(levelLabel)}</span>
    </div>
    ${tool.desc ? `<div class="state" style="margin-top:4px">${escapeHtml(tool.desc)}</div>` : ''}`;
  bar.hidden = false;
}

// ---------------------------------------------------------------------------
// 发送 / 停止
// ---------------------------------------------------------------------------
async function sendMessage() {
  if (!S.conv || S.readOnly) return;
  const input = $('input');
  const content = input.value.trim();
  const images = [...S.images];
  if (!content && !images.length) return;
  input.value = '';
  input.style.height = 'auto';
  S.images = [];
  renderImagePreview();
  S.unread = 0;
  S.atBottom = true;
  $('scrollBottom').classList.remove('show');

  const userMsg = {
    id: 'pending-' + Date.now(),
    role: 'user',
    content: content || '（图片）',
    created_at: Math.floor(Date.now() / 1000),
  };
  S.messages.push(userMsg);
  renderMessages(true);

  S.streaming = { convId: S.conv.id, text: '', el: null };
  ensureStreamingPlaceholder(S.conv.id);

  $('sendBtn').hidden = true;
  $('stopBtn').hidden = false;
  try {
    await api(`/api/conversations/${encodeURIComponent(S.conv.id)}/stream`, {
      method: 'POST',
      body: JSON.stringify(images.length ? { content, images } : { content }),
    });
  } catch (e) {
    endStreaming();
    toast('发送失败：' + e.message, 'err');
  }
}

async function stopChat() {
  if (!S.conv) return;
  try { await api(`/api/conversations/${encodeURIComponent(S.conv.id)}/stop`, { method: 'POST', body: '{}' }); }
  catch (e) { toast('停止失败：' + e.message, 'err'); }
}

// ---------------------------------------------------------------------------
// 待处理卡片
// ---------------------------------------------------------------------------
function renderPendingCards() {
  const wrap = $('pendingCards');
  const items = S.conv ? S.pendingMap[S.conv.id] || [] : [];
  wrap.innerHTML = '';
  items.forEach((p) => wrap.appendChild(pendingCard(p)));
  renderConvList();
}

function pendingCard(p) {
  const div = document.createElement('div');
  div.className = 'pc ' + p.kind;
  div.dataset.rid = p.request_id;
  let body = '';
  if (p.kind === 'approval') {
    body = `<div class="pc-title">${escapeHtml(p.tool || '')}</div>
            <div class="pc-body">${escapeHtml(p.args || '')}</div>
            <div class="pc-actions">
              <button class="pc-btn ok" data-ok="1">允许</button>
              <button class="pc-btn no" data-ok="0">拒绝</button>
              <button class="pc-btn no" data-ok="1" data-remember="1">始终允许</button>
            </div>`;
  } else if (p.kind === 'plan') {
    body = `<div class="pc-title">任务计划</div>
            <div class="pc-body">${escapeHtml(p.plan || '')}</div>
            <div class="pc-actions">
              <button class="pc-btn ok" data-ok="1">批准</button>
              <button class="pc-btn no" data-ok="0">拒绝</button>
            </div>`;
  } else if (p.kind === 'ask') {
    const opts = (p.options || []).map((o) => `<button class="pc-opt" data-opt="${escapeHtml(o)}">${escapeHtml(o)}</button>`).join('');
    body = `<div class="pc-title">Agent 提问</div>
            <div class="pc-body">${escapeHtml(p.question || '')}</div>
            ${opts ? `<div class="pc-opts">${opts}</div>` : ''}
            <input class="pc-input" placeholder="输入回答（留空跳过）">
            <div class="pc-actions"><button class="pc-btn ask">提交回答</button></div>`;
  }
  div.innerHTML = `<div class="pc-head"><span class="pc-dot"></span>${p.kind === 'approval' ? '工具权限请求' : p.kind === 'plan' ? '计划审查' : 'Agent 提问'}</div>${body}`;
  if (S.readOnly) {
    const acts = div.querySelector('.pc-actions');
    if (acts) acts.innerHTML = '<span class="pc-hint">只读模式，请在桌面端处理</span>';
  }
  const optBtns = div.querySelectorAll('.pc-opt');
  let selectedOpt = '';
  optBtns.forEach((b) => {
    b.addEventListener('click', () => {
      optBtns.forEach((x) => x.classList.remove('selected'));
      b.classList.add('selected');
      selectedOpt = b.dataset.opt;
    });
  });
  div.addEventListener('click', (e) => {
    const okBtn = e.target.closest('[data-ok]');
    if (okBtn) {
      const approved = okBtn.dataset.ok === '1';
      const remember = okBtn.dataset.remember === '1';
      resolvePending(p, { kind: 'approval', approved, remember, scope: remember ? 'session' : undefined });
      return;
    }
    const optBtn = e.target.closest('[data-opt]');
    if (optBtn) { resolvePending(p, { kind: 'ask', answer: optBtn.dataset.opt }); return; }
    const askBtn = e.target.closest('.pc-btn.ask');
    if (askBtn) {
      const input = div.querySelector('.pc-input');
      resolvePending(p, { kind: 'ask', answer: (input && input.value.trim()) || selectedOpt });
    }
  });
  return div;
}

async function resolvePending(p, payload) {
  try {
    await api(`/api/approvals/${encodeURIComponent(p.request_id)}`, {
      method: 'POST',
      body: JSON.stringify({ ...payload, approved: payload.approved, conversation_id: S.conv && S.conv.id }),
    });
    const arr = S.pendingMap[p.conversation_id];
    if (arr) {
      S.pendingMap[p.conversation_id] = arr.filter((x) => x.request_id !== p.request_id);
    }
    renderPendingCards();
    toast('已提交', 'ok');
  } catch (e) {
    toast('操作失败：' + e.message, 'err');
  }
}

// ---------------------------------------------------------------------------
// 文件查看
// ---------------------------------------------------------------------------
async function openFile(path) {
  if (!S.conv) return;
  $('fileModal').hidden = false;
  $('fileTitle').textContent = path;
  $('fileBody').innerHTML = '<div class="file-loading">加载中…</div>';
  try {
    const res = await api(
      `/api/projects/${encodeURIComponent(S.conv.project_id)}/file?path=${encodeURIComponent(path)}`
    );
    $('fileBody').textContent = res.content || '（空文件）';
  } catch (e) {
    $('fileBody').textContent = '读取失败：' + e.message;
  }
}

async function openConvFiles() {
  if (!S.conv) return;
  try {
    const files = await api(`/api/conversations/${encodeURIComponent(S.conv.id)}/files`);
    if (!files.length) { toast('该会话暂无修改文件记录'); return; }
    openFileList(files);
  } catch (e) { toast('获取失败：' + e.message, 'err'); }
}

function openFileList(files) {
  $('fileModal').hidden = false;
  $('fileTitle').textContent = '会话修改的文件';
  $('fileBody').innerHTML = files
    .map((f) => `<div class="file-chip" style="display:block;margin-bottom:6px;font-size:12.5px">${escapeHtml(f)}</div>`)
    .join('');
  $('fileBody').querySelectorAll('.file-chip').forEach((chip) => {
    chip.addEventListener('click', () => openFile(chip.textContent.trim()));
  });
}

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------
let searchTimer = null;
async function doSearch(q) {
  const box = $('searchResults');
  const list = $('convList');
  if (!q.trim()) { box.hidden = true; list.hidden = false; return; }
  list.hidden = true;
  box.hidden = false;
  box.innerHTML = '<div class="file-loading">搜索中…</div>';
  try {
    const [project, all] = await Promise.all([
      api(`/api/projects/${encodeURIComponent(S.projectId)}/search?q=${encodeURIComponent(q)}`).catch(() => []),
      api(`/api/search?q=${encodeURIComponent(q)}`).catch(() => []),
    ]);
    const seen = new Set();
    const rows = [];
    [...project, ...all].forEach((h) => {
      const key = h.conversation_id + ':' + h.message_id;
      if (seen.has(key)) return;
      seen.add(key);
      rows.push(h);
    });
    box.innerHTML = rows.length
      ? rows.map((h) => `
        <div class="sr-item" data-cid="${h.conversation_id}" data-mid="${h.message_id}">
          <div class="t">${escapeHtml(h.project_name || '')} · ${escapeHtml(h.conversation_title || '')} · ${fmtTime(h.created_at)}</div>
          <div class="c">${highlight(h.snippet || '', q)}</div>
        </div>`).join('')
      : '<p class="empty"><svg class="empty-ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>未找到相关消息</p>';
    box.querySelectorAll('.sr-item').forEach((el) => {
      el.addEventListener('click', () => previewSearchResult(el.dataset.cid, el.dataset.mid, q));
    });
  } catch (e) {
    box.innerHTML = `<p class="empty">搜索失败：${escapeHtml(e.message)}</p>`;
  }
}

async function previewSearchResult(cid, mid, q) {
  try {
    const msgs = await api(`/api/conversations/${encodeURIComponent(cid)}/messages?limit=20`);
    const arr = (msgs.messages || []);
    const idx = arr.findIndex((m) => String(m.id) === String(mid));
    if (idx < 0) { jumpToConversation(cid); return; }
    const conv = S.convs.find((c) => c.id === cid);
    const title = (conv && conv.title) || (arr[0] && '会话');
    $('previewTitle').textContent = title || '会话';
    $('previewMeta').textContent = fmtTime(arr[idx].created_at) + ' · ' + (arr[idx].role === 'user' ? '我' : 'AI');
    $('previewBody').innerHTML = highlight(arr[idx].content || '', q);
    $('searchPreview').hidden = false;
    $('searchPreview').dataset.cid = cid;
  } catch (e) {
    jumpToConversation(cid);
  }
}

async function jumpToConversation(cid) {
  let conv = S.convs.find((c) => c.id === cid);
  if (!conv) {
    try {
      const all = S.projects.length
        ? (await Promise.all(S.projects.map((p) =>
            api(`/api/projects/${encodeURIComponent(p.id)}/conversations`).catch(() => [])
          ))).flat()
        : [];
      conv = all.find((c) => c.id === cid) || null;
    } catch { /* ignore */ }
  }
  if (!conv) { toast('会话不存在', 'err'); return; }
  $('searchInput').value = '';
  $('searchResults').hidden = true;
  $('convList').hidden = false;
  $('searchPreview').hidden = true;
  if (S.projectId !== conv.project_id) {
    S.projectId = conv.project_id;
    $('projectSelect').value = conv.project_id;
    await loadConvs(S.projectId);
  }
  openConv(conv);
}

// ---------------------------------------------------------------------------
// 会话管理（菜单）
// ---------------------------------------------------------------------------
function toggleMenu() {
  if (S.readOnly) {
    document.querySelectorAll('#chatMenu .write-only').forEach((b) => { b.hidden = true; });
  } else {
    document.querySelectorAll('#chatMenu .write-only').forEach((b) => { b.hidden = false; });
  }
  const menu = $('chatMenu');
  menu.hidden = !menu.hidden;
}

async function menuAction(act) {
  if (!S.conv) return;
  const cid = encodeURIComponent(S.conv.id);
  try {
    if (act === 'rename') {
      const title = await dlg({
        title: '重命名会话', message: '输入新标题：',
        withInput: true, inputValue: S.conv.title || '',
        inputPlaceholder: '会话标题', okText: '保存',
      });
      if (title === null || title === undefined) return;
      const trimmed = String(title).trim();
      if (!trimmed) { toast('标题不能为空', 'err'); return; }
      await api(`/api/conversations/${cid}/rename`, { method: 'POST', body: JSON.stringify({ title: trimmed }) });
      S.conv.title = trimmed;
      $('chatTitle').textContent = trimmed;
      toast('已重命名', 'ok');
    } else if (act === 'pin') {
      const r = await api(`/api/conversations/${cid}/pin`, { method: 'POST', body: JSON.stringify({ pinned: !S.conv.is_pinned }) });
      S.conv.is_pinned = !!r.is_pinned;
      syncMenuLabels();
      toast(S.conv.is_pinned ? '已置顶' : '已取消置顶', 'ok');
    } else if (act === 'archive') {
      const r = await api(`/api/conversations/${cid}/archive`, { method: 'POST', body: JSON.stringify({ archived: !S.conv.archived }) });
      S.conv.archived = !!r.archived;
      syncMenuLabels();
      toast(S.conv.archived ? '已归档' : '已取消归档', 'ok');
    } else if (act === 'files') {
      await openConvFiles();
    } else if (act === 'delete') {
      const ok = await dlg({
        title: '删除会话', message: '确定删除该会话吗？此操作不可恢复！',
        okText: '删除', danger: true, cancelText: '取消',
      });
      if (!ok) return;
      await api(`/api/conversations/${cid}/delete`, { method: 'POST', body: '{}' });
      goBack();
      toast('已删除', 'ok');
    }
  } catch (e) { toast('操作失败：' + e.message, 'err'); }
  $('chatMenu').hidden = true;
}

async function goBack() {
  $('chatMenu').hidden = true;
  showView('list');
  S.conv = null;
  S.messages = [];
  S.streaming = null;
  S.unread = 0;
  $('searchInput').value = '';
  $('searchResults').hidden = true;
  $('convList').hidden = false;
  await loadConvs(S.projectId);
}

// ---------------------------------------------------------------------------
// SSE
// ---------------------------------------------------------------------------
function connectSSE() {
  if (window._devMock) return;  // dev mock 模式无 SSE
  if (S.es || !S.token) return;
  const es = new EventSource(
    `/api/events?token=${encodeURIComponent(S.token)}&ua=${encodeURIComponent(navigator.userAgent)}`
  );
  S.es = es;
  const dot = $('connDot');
  const setConn = (state) => {
    if (!dot) return;
    dot.classList.toggle('on', state === 'on');
    dot.classList.toggle('off', state === 'off');
  };
  setConn('off');
  es.onopen = () => { setConn('on'); refreshPending(); };
  const on = (name, fn) => es.addEventListener(name, (e) => {
    try { fn(JSON.parse(e.data)); } catch { /* ignore */ }
  });
  const acceptsRun = (p) => !p.run_id || !S.streaming || S.streaming.convId !== p.conversation_id
    || S.streaming.runId === p.run_id;
  on('chat-run-started', (p) => {
    if (S.streaming && S.streaming.convId === p.conversation_id) {
      S.streaming.runId = p.run_id;
    } else if (S.conv && S.conv.id === p.conversation_id) {
      // 排队消息自动续跑时上一轮已 endStreaming；新代次必须重建占位和停止按钮。
      S.streaming = { convId: p.conversation_id, runId: p.run_id, text: '', el: null };
      ensureStreamingPlaceholder(p.conversation_id);
      $('sendBtn').hidden = true;
      $('stopBtn').hidden = false;
    }
  });
  on('chat-stream', (p) => {
    if (acceptsRun(p)) appendStream(p.conversation_id, p.delta || '');
  });
  on('chat-stream-batch', (p) => {
    if (acceptsRun(p) && p.content) appendStream(p.conversation_id, p.content);
  });
  on('chat-reasoning', () => { /* keep alive */ });
  on('chat-done', (p) => {
    if (!acceptsRun(p)) return;
    if (S.conv && p.conversation_id === S.conv.id) {
      endStreaming();
      reloadMessages().catch(() => {});
    }
    maybeNotify(p.conversation_id, '任务完成', (p.message && (p.message.content || '').slice(0, 60)) || '');
  });
  on('chat-error', (p) => {
    if (!acceptsRun(p)) return;
    if (S.conv && p.conversation_id === S.conv.id) {
      endStreaming();
      const list = $('msgList');
      const div = document.createElement('div');
      div.className = 'msg assistant';
      div.innerHTML = `<div class="bubble" style="color:var(--danger);background:var(--danger-soft);border-color:transparent">${escapeHtml(p.error || '出错了')}</div>`;
      list.appendChild(div);
      list.scrollTop = list.scrollHeight;
    }
    maybeNotify(p.conversation_id, '任务出错', p.title || p.error || '');
  });
  on('chat-stopped', (p) => {
    if (!acceptsRun(p)) return;
    if (S.conv && p.conversation_id === S.conv.id) {
      endStreaming();
      reloadMessages().catch(() => {});
      toast('已停止', 'ok');
    }
  });
  on('chat-tool-start', (p) => {
    if (!acceptsRun(p)) return;
    S.tool = p;
    renderTool(p.conversation_id, p);
  });
  on('chat-tool-done', (p) => {
    if (!acceptsRun(p)) return;
    if (S.tool && S.tool.conversation_id === p.conversation_id) {
      S.tool = null;
      $('toolBar').hidden = true;
    }
  });
  on('chat-tool-approval', (p) => {
    addPending({ ...p, kind: 'approval' });
    maybeNotify(p.conversation_id, '需要审批', `工具 ${p.tool || ''} 等待允许`);
  });
  on('chat-plan', (p) => {
    addPending({ ...p, kind: 'plan' });
    maybeNotify(p.conversation_id, '计划待批准', (p.plan || '').slice(0, 60));
  });
  on('chat-ask', (p) => {
    addPending({ ...p, kind: 'ask' });
    maybeNotify(p.conversation_id, 'Agent 提问', p.question || '');
  });
  on('conversation-renamed', (p) => {
    if (S.conv && p.conversation_id === S.conv.id) {
      S.conv.title = p.title;
      $('chatTitle').textContent = p.title;
      syncMenuLabels();
    }
    loadConvs(S.projectId).catch(() => {});
  });
  on('projects-changed', () => loadProjects().then(() => loadConvs(S.projectId)).catch(() => {}));
  on('session-expired', () => { logout('令牌已过期或已被撤销'); });
  es.onerror = () => {
    setConn('off');
    if (S.streaming && S.conv) endStreaming();
  };
}

function addPending(p) {
  (S.pendingMap[p.conversation_id] = S.pendingMap[p.conversation_id] || []).push(p);
  if (S.conv && p.conversation_id === S.conv.id) {
    renderPendingCards();
    toast('有新待处理项');
  } else {
    renderConvList();
  }
}

// ---------------------------------------------------------------------------
// 移动端体验
// ---------------------------------------------------------------------------
function initMobileUX() {
  if (!window.visualViewport) return;
  const vv = window.visualViewport;
  const updateVH = () => {
    const h = Math.max(vv.height, 200);
    document.documentElement.style.setProperty('--vv-height', h + 'px');
    const list = $('msgList');
    if (list && S.atBottom) list.scrollTop = list.scrollHeight;
  };
  vv.addEventListener('resize', updateVH);
  vv.addEventListener('scroll', updateVH);
  updateVH();
}

function initSwipeBack() {
  const view = $('view-chat');
  if (!view) return;
  let startX = 0, startY = 0, startT = 0, tracking = false;
  const EDGE = 24, MIN_DX = 80, MAX_DY = 60;
  view.addEventListener('touchstart', (e) => {
    if (e.touches.length !== 1) return;
    if (e.touches[0].clientX > EDGE) return;
    if (e.target.closest('input, textarea, .menu, .modal, .pc')) return;
    startX = e.touches[0].clientX;
    startY = e.touches[0].clientY;
    startT = Date.now();
    tracking = true;
  }, { passive: true });
  view.addEventListener('touchmove', (e) => {
    if (!tracking) return;
    const dx = e.touches[0].clientX - startX;
    const dy = Math.abs(e.touches[0].clientY - startY);
    if (dy > MAX_DY) { tracking = false; return; }
    if (dx > 0 && dx < window.innerWidth) {
      view.style.transition = 'none';
      view.style.transform = `translateX(${dx * 0.6}px)`;
      view.style.opacity = String(1 - dx / 600);
    }
  }, { passive: true });
  view.addEventListener('touchend', (e) => {
    if (!tracking) return;
    tracking = false;
    const dx = (e.changedTouches[0].clientX - startX);
    const dt = Date.now() - startT;
    view.style.transition = 'transform 0.2s var(--ease), opacity 0.2s var(--ease)';
    if (dx > MIN_DX && dt < 600) {
      view.style.transform = 'translateX(100%)';
      view.style.opacity = '0';
      setTimeout(() => {
        view.style.transition = '';
        view.style.transform = '';
        view.style.opacity = '';
        goBack();
      }, 200);
    } else {
      view.style.transform = '';
      view.style.opacity = '';
      setTimeout(() => { view.style.transition = ''; }, 200);
    }
  }, { passive: true });
}

function initPullRefresh() {
  const list = $('convList');
  if (!list) return;
  const parent = list.parentElement;
  let startY = 0, tracking = false, refreshing = false;
  const MAX = 80;
  const indicator = document.createElement('div');
  indicator.className = 'pull-indicator';
  indicator.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg><span>下拉刷新</span>';
  parent.insertBefore(indicator, list);
  list.addEventListener('touchstart', (e) => {
    if (refreshing) return;
    if (list.scrollTop > 0) return;
    if (e.touches.length !== 1) return;
    startY = e.touches[0].clientY;
    tracking = true;
  }, { passive: true });
  list.addEventListener('touchmove', (e) => {
    if (!tracking) return;
    const dy = e.touches[0].clientY - startY;
    if (dy <= 0) { indicator.style.height = '0px'; return; }
    if (dy > MAX * 2) return;
    const h = Math.min(dy * 0.4, MAX);
    indicator.style.height = h + 'px';
    const label = indicator.querySelector('span');
    const svg = indicator.querySelector('svg');
    if (h >= MAX) { label.textContent = '释放刷新'; svg.style.transform = 'rotate(180deg)'; }
    else { label.textContent = '下拉刷新'; svg.style.transform = ''; }
  }, { passive: true });
  list.addEventListener('touchend', async () => {
    if (!tracking) return;
    tracking = false;
    const h = parseFloat(indicator.style.height) || 0;
    if (h >= MAX && !refreshing) {
      refreshing = true;
      const label = indicator.querySelector('span');
      const svg = indicator.querySelector('svg');
      label.textContent = '刷新中…';
      svg.style.transform = '';
      svg.style.animation = 'spin 0.7s linear infinite';
      try {
        await loadConvs(S.projectId);
        toast('已刷新', 'ok');
      } finally {
        svg.style.animation = '';
        indicator.style.height = '0px';
        refreshing = false;
      }
    } else {
      indicator.style.height = '0px';
    }
  });
}

// ---------------------------------------------------------------------------
// 事件绑定 + 启动
// ---------------------------------------------------------------------------
function bindEvents() {
  $('loginBtn').addEventListener('click', tryLogin);
  $('newConvBtn').addEventListener('click', newConversation);
  $('refreshBtn').addEventListener('click', () => {
    const btn = $('refreshBtn');
    btn.style.transform = 'rotate(360deg)';
    btn.style.transition = 'transform 0.6s var(--ease)';
    setTimeout(() => { btn.style.transform = ''; btn.style.transition = ''; }, 700);
    loadConvs(S.projectId).then(() => toast('已刷新', 'ok'));
  });
  $('projectSelect').addEventListener('click', openProjectDrawer);
  document.querySelectorAll('#projectDrawer .drawer-close').forEach((b) => {
    b.addEventListener('click', closeProjectDrawer);
  });
  const dbg = document.querySelector('#projectDrawer .drawer-backdrop');
  if (dbg) dbg.addEventListener('click', closeProjectDrawer);
  const psi = $('projectSearchInput');
  if (psi) psi.addEventListener('input', (e) => renderProjectList(e.target.value));
  const ncc = $('newConvClose');
  if (ncc) ncc.addEventListener('click', closeNewConvDialog);
  const nca = $('newConvCancel');
  if (nca) nca.addEventListener('click', closeNewConvDialog);
  const ncs = $('newConvSubmit');
  if (ncs) ncs.addEventListener('click', submitNewConv);
  const ncm = $('newConvModal');
  if (ncm) ncm.addEventListener('click', (e) => { if (e.target === ncm) closeNewConvDialog(); });
  $('searchInput').addEventListener('input', (e) => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => doSearch(e.target.value), 350);
  });
  $('searchInput').addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.target.value = ''; doSearch(''); e.target.blur(); }
  });
  $('backBtn').addEventListener('click', goBack);
  $('chatMenuBtn').addEventListener('click', toggleMenu);
  $('chatMenu').addEventListener('click', (e) => {
    const btn = e.target.closest('button[data-act]');
    if (btn) menuAction(btn.dataset.act);
  });
  $('sendBtn').addEventListener('click', sendMessage);
  $('stopBtn').addEventListener('click', stopChat);
  $('scrollBottom').addEventListener('click', () => {
    $('msgList').scrollTop = $('msgList').scrollHeight;
    S.unread = 0;
    S.atBottom = true;
    updateScrollBottom();
  });
  // 搜索预览弹层
  const pc1 = $('previewClose');
  if (pc1) pc1.addEventListener('click', () => { $('searchPreview').hidden = true; });
  const pc2 = $('previewCancel');
  if (pc2) pc2.addEventListener('click', () => { $('searchPreview').hidden = true; });
  const pc3 = $('previewOpen');
  if (pc3) pc3.addEventListener('click', () => {
    const cid = $('searchPreview').dataset.cid;
    $('searchPreview').hidden = true;
    if (cid) jumpToConversation(cid);
  });
  // 上下文菜单点击外部关闭
  document.addEventListener('click', (e) => {
    const m = $('contextMenu');
    if (m && !m.hidden && !m.contains(e.target)) m.hidden = true;
  });

  initMic();
  initImagePicker();
  $('input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      sendMessage();
    }
  });
  $('input').addEventListener('input', (e) => {
    e.target.style.height = 'auto';
    e.target.style.height = Math.min(e.target.scrollHeight, 120) + 'px';
  });
  $('chatMenu').hidden = true;
  $('fileClose').addEventListener('click', () => { $('fileModal').hidden = true; });
  $('fileModal').addEventListener('click', (e) => { if (e.target === $('fileModal')) $('fileModal').hidden = true; });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if (!$('fileModal').hidden) $('fileModal').hidden = true;
      else if (!$('searchPreview').hidden) $('searchPreview').hidden = true;
      else if (!$('chatMenu').hidden) $('chatMenu').hidden = true;
      else if (!$('contextMenu').hidden) $('contextMenu').hidden = true;
      else if (!$('projectDrawer').hidden) closeProjectDrawer();
    }
  });
  $('msgList').addEventListener('scroll', () => {
    if ($('msgList').scrollTop < 40) loadOlder();
    updateScrollBottom();
  });
}

// ---------------------------------------------------------------------------
// dev mock 调试模式：?dev=1 跳过 token 直接塞假数据进主界面
// ---------------------------------------------------------------------------
async function bootDevMock() {
  const PROJECTS = [
    { id: 'global', name: '全局', path: '', last_active_at: nowSec() },
    { id: 'p1', name: 'sns-backend', path: '~/work/sns-backend', last_active_at: nowSec() - 30 },
    { id: 'p2', name: 'novel-web', path: '~/work/novel-web', last_active_at: nowSec() - 7200 },
    { id: 'p3', name: 'lan-tools', path: '~/work/lan-tools', last_active_at: nowSec() - 86400 },
  ];
  const CONVS = [
    { id: 'c1', title: '登录态验证 + Token 刷新', updated_at: nowSec() - 600, is_pinned: true, archived: false },
    { id: 'c2', title: 'Markdown 渲染兼容性测试', updated_at: nowSec() - 1800, is_pinned: false, archived: false },
    { id: 'c3', title: '老会话（已归档）', updated_at: nowSec() - 86400 * 3, is_pinned: false, archived: true },
    { id: 'c4', title: '今天要做的几件事', updated_at: nowSec() - 200, is_pinned: true, archived: false },
  ];
  const MESSAGES = [
    { id: 'm1', role: 'user', content: '帮我看看这个 bug：用户在移动端登录后 token 偶尔会失效', ts: nowSec() - 700 },
    { id: 'm2', role: 'assistant', content: '好的，先复现一下。我有几个问题：\n\n1. 失效的频率是大约多久一次？\n2. 前后端用的是什么鉴权方式（JWT 过期 / 服务端 session）？\n3. 移动端是 Web 还是 App？\n\n```js\n// 顺便检查一下这段 token 刷新逻辑有没有死循环\nasync function refresh() {\n  const r = await fetch("/api/refresh", { method: "POST" });\n  if (!r.ok) throw new Error("refresh failed");\n  return r.json();\n}\n```', ts: nowSec() - 690 },
    { id: 'm3', role: 'user', content: 'JWT，30 分钟过期，移动 Web。频率大概一两次每天', ts: nowSec() - 680 },
    { id: 'm4', role: 'assistant', content: '明白了，这种偶发问题 80% 是 **并发刷新** 引起的：\n\n- Token 即将过期时，多个请求同时触发刷新\n- 旧 token 进入黑名单前，新请求带着旧 token 又请求了一次\n- 结果：服务端认为是非法 token，主动踢下线\n\n**修复方案**：用一个内存变量 `refreshing` 锁住并发。\n\n```javascript\nlet refreshing = null;\nasync function getToken() {\n  if (isExpired(token)) {\n    if (!refreshing) {\n      refreshing = doRefresh().finally(() => { refreshing = null; });\n    }\n    token = await refreshing;\n  }\n  return token;\n}\n```\n\n需要我帮你写完整版吗？\n\n补充一句：\n如果用第三方库（比如 axios interceptor）通常已经帮你处理好了，不用手写。\n下次遇到类似问题可以先看拦截器是否生效。', ts: nowSec() - 670 },
    { id: 'm5', role: 'user', content: '好，谢谢', ts: nowSec() - 600 },
  ];
  // 重写 api 函数：直接返回 mock 数据
  window.api = async function (path, opts = {}) {
    if (path === '/api/lan/status') return { read_only: false };
    if (path === '/api/projects') return PROJECTS;
    if (path.startsWith('/api/projects/') && path.endsWith('/conversations')) return CONVS;
    if (path.startsWith('/api/projects/') && path.endsWith('/pending')) return [];
    if (path.startsWith('/api/projects/') && !path.includes('/conversations')) return { agents: ['general', 'coder', 'reviewer'], models: ['gpt-4o', 'claude-3.5', 'gpt-4o-mini'] };
    if (path.includes('/messages')) return { messages: MESSAGES, has_more: false };
    if (path.includes('/conversations') && opts.method === 'POST' && !path.includes('/stream') && !path.includes('/messages')) {
      return { id: 'c_new_' + Date.now(), title: '新会话', updated_at: nowSec(), is_pinned: false, archived: false };
    }
    return {};
  };
  // 禁用 SSE
  window._devMock = true;
  // 跳过 token 界面
  S.token = '000000';
  localStorage.setItem(TOKEN_KEY, S.token);
  // 写 conn-dot 状态
  setTimeout(() => {
    const dot = $('connDot');
    if (dot) { dot.classList.add('off'); dot.title = 'dev mock（无实时连接）'; }
  }, 100);
  bindEvents();
  buildTokenGrid();
  initMobileUX();
  initSwipeBack();
  initPullRefresh();
  showView('list');
  await enterList();
}

function nowSec() { return Math.floor(Date.now() / 1000); }

function boot() {
  // ?dev=1 调试模式：跳过真实连接，用假数据进主界面
  if (new URLSearchParams(location.search).get('dev') === '1') {
    bootDevMock().catch((e) => {
      console.error('[dev mock]', e);
      showView('login');
    });
    return;
  }
  bindEvents();
  buildTokenGrid();
  initMobileUX();
  initSwipeBack();
  initPullRefresh();
  if (S.token) {
    $('loginHint').textContent = '';
    connectSSE();
    tryLogin().catch(() => {});
  } else {
    showView('login');
  }
}

boot();
