// ====== Kenjaku Geto-Web ======
// Single-page UI for the Kenjaku RAG search engine.
// Talks to POST /api/v1/search (both SSE and JSON), GET /api/v1/autocomplete,
// GET /api/v1/top-searches, POST /api/v1/feedback.

// ====== i18n ======
// Client-side string table keyed by BCP-47 locale. Empty userLocale ("Auto")
// falls back to English. Strings are looked up via t(key).
var TRANSLATIONS = {
  en: {
    app_title: 'Kenjaku AI',
    search_greeting: 'What can I help you with?',
    top_searches: 'Top searches',
    searching: 'Searching',
    debug: 'Debug',
    ask_followup: 'Ask a follow-up',
    debug_auth_title: 'Debug: Auth Token (staging/production)',
    debug_raw_json: 'Debug: Raw JSON',
    bearer_token: 'Bearer Token',
    bearer_placeholder: 'Paste token for non-local environments',
    copy: 'Copy',
    helpful: 'Helpful',
    not_helpful: 'Not helpful',
    source_one: 'Source',
    source_other: 'Sources',
    sources_title: 'Sources',
    help_us_improve: 'Help us improve',
    tell_us_more: 'Tell us more',
    submit: 'Submit',
    thanks_feedback: 'Thanks for your feedback!',
    locale_auto: 'Auto',
    reason_factually_incorrect: 'Factually incorrect',
    reason_missing_key_information: 'Missing key information',
    reason_ignored_or_refused_instructions: 'Ignored or refused instructions',
    reason_harmful_or_offensive: 'Harmful or offensive',
  },
  zh: {
    app_title: 'Kenjaku AI',
    search_greeting: '我可以帮你什么?',
    top_searches: '热门搜索',
    searching: '搜索中',
    debug: '调试',
    ask_followup: '继续提问',
    debug_auth_title: '调试: 认证令牌 (staging/production)',
    debug_raw_json: '调试: 原始 JSON',
    bearer_token: 'Bearer 令牌',
    bearer_placeholder: '粘贴非本地环境的令牌',
    copy: '复制',
    helpful: '有帮助',
    not_helpful: '没帮助',
    source_one: '来源',
    source_other: '来源',
    sources_title: '来源',
    help_us_improve: '帮助我们改进',
    tell_us_more: '告诉我们更多',
    submit: '提交',
    thanks_feedback: '感谢您的反馈!',
    locale_auto: '自动',
    reason_factually_incorrect: '事实错误',
    reason_missing_key_information: '缺少关键信息',
    reason_ignored_or_refused_instructions: '忽略或拒绝指令',
    reason_harmful_or_offensive: '有害或冒犯',
  },
  'zh-TW': {
    app_title: 'Kenjaku AI',
    search_greeting: '我可以幫你什麼?',
    top_searches: '熱門搜尋',
    searching: '搜尋中',
    debug: '除錯',
    ask_followup: '繼續提問',
    debug_auth_title: '除錯: 認證權杖 (staging/production)',
    debug_raw_json: '除錯: 原始 JSON',
    bearer_token: 'Bearer 權杖',
    bearer_placeholder: '貼上非本地環境的權杖',
    copy: '複製',
    helpful: '有幫助',
    not_helpful: '沒幫助',
    source_one: '來源',
    source_other: '來源',
    sources_title: '來源',
    help_us_improve: '幫助我們改進',
    tell_us_more: '告訴我們更多',
    submit: '送出',
    thanks_feedback: '感謝您的回饋!',
    locale_auto: '自動',
    reason_factually_incorrect: '事實錯誤',
    reason_missing_key_information: '缺少關鍵資訊',
    reason_ignored_or_refused_instructions: '忽略或拒絕指令',
    reason_harmful_or_offensive: '有害或冒犯',
  },
  ja: {
    app_title: 'Kenjaku AI',
    search_greeting: '何かお手伝いできますか?',
    top_searches: '人気の検索',
    searching: '検索中',
    debug: 'デバッグ',
    ask_followup: '続けて質問する',
    debug_auth_title: 'デバッグ: 認証トークン (staging/production)',
    debug_raw_json: 'デバッグ: Raw JSON',
    bearer_token: 'Bearer トークン',
    bearer_placeholder: '非ローカル環境のトークンを貼り付け',
    copy: 'コピー',
    helpful: '役に立った',
    not_helpful: '役に立たない',
    source_one: 'ソース',
    source_other: 'ソース',
    sources_title: 'ソース',
    help_us_improve: '改善にご協力ください',
    tell_us_more: '詳しく教えてください',
    submit: '送信',
    thanks_feedback: 'フィードバックありがとうございます!',
    locale_auto: '自動',
    reason_factually_incorrect: '事実と異なる',
    reason_missing_key_information: '重要な情報が不足',
    reason_ignored_or_refused_instructions: '指示を無視/拒否',
    reason_harmful_or_offensive: '有害または不適切',
  },
  ko: {
    app_title: 'Kenjaku AI',
    search_greeting: '무엇을 도와드릴까요?',
    top_searches: '인기 검색',
    searching: '검색 중',
    debug: '디버그',
    ask_followup: '추가 질문하기',
    debug_auth_title: '디버그: 인증 토큰 (staging/production)',
    debug_raw_json: '디버그: Raw JSON',
    bearer_token: 'Bearer 토큰',
    bearer_placeholder: '비로컬 환경의 토큰 붙여넣기',
    copy: '복사',
    helpful: '도움됨',
    not_helpful: '도움 안됨',
    source_one: '출처',
    source_other: '출처',
    sources_title: '출처',
    help_us_improve: '개선을 도와주세요',
    tell_us_more: '자세히 알려주세요',
    submit: '제출',
    thanks_feedback: '피드백 감사합니다!',
    locale_auto: '자동',
    reason_factually_incorrect: '사실과 다름',
    reason_missing_key_information: '핵심 정보 누락',
    reason_ignored_or_refused_instructions: '지시 무시/거부',
    reason_harmful_or_offensive: '유해하거나 불쾌함',
  },
  de: {
    app_title: 'Kenjaku AI',
    search_greeting: 'Wobei kann ich helfen?',
    top_searches: 'Top-Suchanfragen',
    searching: 'Suche läuft',
    debug: 'Debug',
    ask_followup: 'Folgefrage stellen',
    debug_auth_title: 'Debug: Auth-Token (staging/production)',
    debug_raw_json: 'Debug: Raw JSON',
    bearer_token: 'Bearer-Token',
    bearer_placeholder: 'Token für Nicht-Lokal-Umgebungen einfügen',
    copy: 'Kopieren',
    helpful: 'Hilfreich',
    not_helpful: 'Nicht hilfreich',
    source_one: 'Quelle',
    source_other: 'Quellen',
    sources_title: 'Quellen',
    help_us_improve: 'Hilf uns besser zu werden',
    tell_us_more: 'Mehr erzählen',
    submit: 'Senden',
    thanks_feedback: 'Danke für dein Feedback!',
    locale_auto: 'Auto',
    reason_factually_incorrect: 'Sachlich falsch',
    reason_missing_key_information: 'Wichtige Informationen fehlen',
    reason_ignored_or_refused_instructions: 'Anweisungen ignoriert/abgelehnt',
    reason_harmful_or_offensive: 'Schädlich oder beleidigend',
  },
  fr: {
    app_title: 'Kenjaku AI',
    search_greeting: 'Comment puis-je vous aider ?',
    top_searches: 'Recherches populaires',
    searching: 'Recherche en cours',
    debug: 'Débogage',
    ask_followup: 'Poser une question',
    debug_auth_title: 'Débogage : jeton d\'auth (staging/production)',
    debug_raw_json: 'Débogage : JSON brut',
    bearer_token: 'Jeton Bearer',
    bearer_placeholder: 'Coller le jeton pour les environnements distants',
    copy: 'Copier',
    helpful: 'Utile',
    not_helpful: 'Pas utile',
    source_one: 'Source',
    source_other: 'Sources',
    sources_title: 'Sources',
    help_us_improve: 'Aidez-nous à nous améliorer',
    tell_us_more: 'Dites-nous en plus',
    submit: 'Envoyer',
    thanks_feedback: 'Merci pour votre retour !',
    locale_auto: 'Auto',
    reason_factually_incorrect: 'Factuellement incorrect',
    reason_missing_key_information: 'Information clé manquante',
    reason_ignored_or_refused_instructions: 'Instructions ignorées/refusées',
    reason_harmful_or_offensive: 'Nuisible ou offensant',
  },
  es: {
    app_title: 'Kenjaku AI',
    search_greeting: '¿En qué puedo ayudarte?',
    top_searches: 'Búsquedas populares',
    searching: 'Buscando',
    debug: 'Depurar',
    ask_followup: 'Hacer otra pregunta',
    debug_auth_title: 'Depurar: token de auth (staging/production)',
    debug_raw_json: 'Depurar: JSON sin procesar',
    bearer_token: 'Token Bearer',
    bearer_placeholder: 'Pega el token para entornos no locales',
    copy: 'Copiar',
    helpful: 'Útil',
    not_helpful: 'No útil',
    source_one: 'Fuente',
    source_other: 'Fuentes',
    sources_title: 'Fuentes',
    help_us_improve: 'Ayúdanos a mejorar',
    tell_us_more: 'Cuéntanos más',
    submit: 'Enviar',
    thanks_feedback: '¡Gracias por tus comentarios!',
    locale_auto: 'Auto',
    reason_factually_incorrect: 'Incorrecto fácticamente',
    reason_missing_key_information: 'Falta información clave',
    reason_ignored_or_refused_instructions: 'Instrucciones ignoradas/rechazadas',
    reason_harmful_or_offensive: 'Dañino u ofensivo',
  },
};

function t(key) {
  var loc = (typeof userLocale !== 'undefined' && userLocale) ? userLocale : 'en';
  var table = TRANSLATIONS[loc] || TRANSLATIONS.en;
  return table[key] || TRANSLATIONS.en[key] || key;
}

function applyI18n() {
  var nodes = document.querySelectorAll('[data-i18n]');
  for (var i = 0; i < nodes.length; i++) {
    nodes[i].textContent = t(nodes[i].getAttribute('data-i18n'));
  }
  var phNodes = document.querySelectorAll('[data-i18n-placeholder]');
  for (var j = 0; j < phNodes.length; j++) {
    phNodes[j].placeholder = t(phNodes[j].getAttribute('data-i18n-placeholder'));
  }
  var titleNodes = document.querySelectorAll('[data-i18n-title]');
  for (var k = 0; k < titleNodes.length; k++) {
    titleNodes[k].title = t(titleNodes[k].getAttribute('data-i18n-title'));
  }
}

// ====== Environment Switcher ======
// When running behind Nginx (Docker Compose on localhost:3000) we use relative
// proxy paths to avoid CORS. Otherwise we call the backend directly.
// geto-web is always served through nginx (localhost:3000 in dev, or any
// public hostname like kenjaku.org via Cloudflare Tunnel). In both cases
// the /api/v1, /proxy/staging/*, /proxy/production/* paths are reverse-
// proxied to the right backend. The only case where JS would need an
// absolute URL is opening app.js directly via file:// — not a supported mode.
var ENV_CONFIGS = {
  local:      { label: 'Local',      base: '/api/v1' },
  staging:    { label: 'Staging',    base: '/proxy/staging/api/v1' },
  production: { label: 'Production', base: '/proxy/production/api/v1' }
};

var currentEnv = localStorage.getItem('env') || 'local';
var API_BASE = ENV_CONFIGS[currentEnv].base;

var envSelect = document.getElementById('envSelect');
var envLabel = document.getElementById('envLabel');

function applyEnv(env) {
  currentEnv = env;
  API_BASE = ENV_CONFIGS[env].base;
  envLabel.textContent = ENV_CONFIGS[env].label;
  document.title = 'Kenjaku ' + ENV_CONFIGS[env].label + ' AI';
  localStorage.setItem('env', env);
  envSelect.value = env;
  loadPills();
}

envSelect.value = currentEnv;
envLabel.textContent = ENV_CONFIGS[currentEnv].label;
document.title = 'Kenjaku ' + ENV_CONFIGS[currentEnv].label + ' AI';
envSelect.addEventListener('change', function() { applyEnv(this.value); });

// ====== Bearer Token (for staging/production — wired up, not sent if empty) ======
var bearerTokenInput = document.getElementById('bearerToken');
var savedToken = localStorage.getItem('bearerToken') || '';
if (bearerTokenInput) {
  bearerTokenInput.value = savedToken;
  bearerTokenInput.addEventListener('input', function() {
    localStorage.setItem('bearerToken', this.value);
  });
}

function getAuthHeaders(extraHeaders) {
  var headers = { 'Content-Type': 'application/json' };
  if (currentEnv !== 'local') {
    var token = bearerTokenInput ? bearerTokenInput.value.trim() : '';
    if (token) headers['Authorization'] = 'Bearer ' + token;
  }
  // Send `X-Session-Id` only when we have one captured from a previous
  // server response. First query of a page session sends nothing — the
  // server generates a UUID and returns it in the response/SSE start
  // metadata, which we then capture into `sessionId` for subsequent
  // requests. A page reload resets `sessionId`, so the server issues a
  // fresh UUID — that's the "new conversation per refresh" semantics.
  if (typeof sessionId === 'string' && sessionId) {
    headers['X-Session-Id'] = sessionId;
  }
  if (extraHeaders) {
    for (var k in extraHeaders) {
      if (Object.prototype.hasOwnProperty.call(extraHeaders, k)) {
        headers[k] = extraHeaders[k];
      }
    }
  }
  return headers;
}

function getAuthHeadersWithAccept(extraHeaders) {
  var headers = getAuthHeaders(extraHeaders);
  headers['Accept'] = 'text/event-stream, application/json';
  return headers;
}

// ====== DOM ======
var searchInput = document.getElementById('searchInput');
var searchBtn = document.getElementById('searchBtn');
var resultsDiv = document.getElementById('results');
var rawJsonPre = document.getElementById('rawJson');
var pillsDiv = document.getElementById('pills');
var queryEcho = document.getElementById('queryEcho');
var searchView = document.getElementById('searchView');
var resultsView = document.getElementById('resultsView');
var searchStatus = document.getElementById('searchStatus');
var progressBar = document.getElementById('progressBar');
var debugInfo = document.getElementById('debugInfo');
var scrollArea = document.getElementById('scrollArea');

// ====== Locale Switcher ======
// `userLocale` is the explicit user choice, persisted across sessions. When
// set we send `?locale=` on all GETs; when empty we omit the param and let
// the server resolve via session memory / Accept-Language / default.
var userLocale = localStorage.getItem('kenjaku_locale') || '';
var localeSwitcher = document.getElementById('localeSwitcher');
var localeLabel = document.getElementById('localeLabel');
function renderLocaleLabel() {
  if (!localeLabel) return;
  if (!userLocale) { localeLabel.textContent = t('locale_auto'); return; }
  var opt = localeSwitcher && localeSwitcher.querySelector('option[value="' + userLocale + '"]');
  localeLabel.textContent = opt ? opt.textContent : userLocale;
}
if (localeSwitcher) {
  localeSwitcher.value = userLocale;
  renderLocaleLabel();
  localeSwitcher.addEventListener('change', function() {
    userLocale = this.value || '';
    if (userLocale) {
      localStorage.setItem('kenjaku_locale', userLocale);
    } else {
      localStorage.removeItem('kenjaku_locale');
    }
    applyI18n();
    renderLocaleLabel();
    loadPills();
  });
}
applyI18n();

// Session id lives in sessionStorage — persists across refreshes in
// the same tab, cleared automatically when the tab is closed. That
// matches the "one conversation per browser tab" mental model.
// Clean up stale localStorage keys from earlier versions that wrote
// `kenjaku_device_id` / `sessionId` there — harmless but noisy in
// DevTools otherwise.
try {
  localStorage.removeItem('kenjaku_device_id');
  localStorage.removeItem('sessionId');
} catch (_) { /* ignore — private mode throws */ }

function localeQueryString() {
  return userLocale ? ('&locale=' + encodeURIComponent(userLocale)) : '';
}

// ====== Session / Feedback State ======
// Session id is captured from the first server response and mirrored
// into sessionStorage so it survives a page refresh in the same tab
// and clears automatically when the tab closes (that's just how
// sessionStorage works). Refreshing the page within the same tab
// therefore CONTINUES the conversation; open a new tab or close and
// reopen to get a fresh session.
//
// `lastRequestId` is transient per-request state — no need to persist.
var feedbackState = {};                  // request_id -> 'like' | 'dislike' | null
var lastRequestId = null;
var lastQuery = null;
var lastResponseText = null;
var sessionId = (function () {
  try { return sessionStorage.getItem('kenjaku_session_id') || null; }
  catch (_) { return null; }
})();
var currentAbortController = null;

function setSessionId(id) {
  sessionId = id || null;
  try {
    if (sessionId) sessionStorage.setItem('kenjaku_session_id', sessionId);
    else sessionStorage.removeItem('kenjaku_session_id');
  } catch (_) { /* private mode */ }
}

// Wipe the conversation. Called by the back button — the next search
// will be sent without `X-Session-Id`, the server will generate a
// fresh UUID, and we capture it on the response.
function clearConversationState() {
  setSessionId(null);
  lastRequestId = null;
  lastQuery = null;
  lastResponseText = null;
  feedbackState = {};
}

// Reason categories match the server-seeded rows in `reason_categories` table.
// IDs here are the serial PK values from `migrations/20260406000001_initial.up.sql`.
var DISLIKE_REASONS = [
  { id: 1, slug: 'factually_incorrect',             i18n: 'reason_factually_incorrect' },
  { id: 2, slug: 'missing_key_information',         i18n: 'reason_missing_key_information' },
  { id: 3, slug: 'ignored_or_refused_instructions', i18n: 'reason_ignored_or_refused_instructions' },
  { id: 4, slug: 'harmful_or_offensive',            i18n: 'reason_harmful_or_offensive' },
];

// ====== Send / Stop Button ======
var sendIconSvg = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></svg>';
var stopIconSvg = '<svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>';

function setButtonStop() { setHtml(searchBtn, stopIconSvg); searchBtn.classList.add('stop'); }
function setButtonSend() { setHtml(searchBtn, sendIconSvg); searchBtn.classList.remove('stop'); }

function abortCurrentSearch() {
  if (currentAbortController) { currentAbortController.abort(); currentAbortController = null; }
  hideLoading();
  setButtonSend();
}

// ====== View transitions ======
function showResultsView(query) {
  searchView.style.display = 'none';
  resultsView.style.display = 'block';
  queryEcho.textContent = query;
  searchStatus.style.display = 'inline-flex';
  progressBar.classList.remove('active');
  void progressBar.offsetWidth;
  progressBar.classList.add('active');
  clearHtml(resultsDiv);
  debugInfo.style.display = 'none';
  debugInfo.removeAttribute('open');
  scrollArea.scrollTop = 0;
  setButtonStop();
}

function hideLoading() {
  searchStatus.style.display = 'none';
  progressBar.classList.remove('active');
  setButtonSend();
  currentAbortController = null;
}

function showSearchView() {
  searchView.style.display = 'block';
  resultsView.style.display = 'none';
  searchInput.value = '';
  searchInput.placeholder = t('ask_followup');
  searchInput.style.height = 'auto';
  if (typeof updateCharCounter === 'function') updateCharCounter();
}

// ====== Raw JSON helper ======
function toRawJson(obj) {
  return JSON.stringify(obj, null, 2);
}

// ====== HTML escape ======
function escapeHtml(str) {
  var div = document.createElement('div');
  div.textContent = str == null ? '' : String(str);
  // nosemgrep: javascript.browser.security.insecure-document-method.insecure-document-method
  return div.innerHTML;
}

// ====== setHtml: centralized innerHTML sink ======
// All UI templates built in this file pass user-controlled values through
// `escapeHtml()` or `inlineMarkdown()` before concatenation. This helper is
// the single trust boundary — semgrep is suppressed here only.
function setHtml(el, html) {
  if (!el) return;
  // nosemgrep: javascript.browser.security.insecure-document-method.insecure-document-method
  el.innerHTML = html;
}
function clearHtml(el) { setHtml(el, ''); }

// ====== Markdown Rendering ======
function renderMarkdownBlocks(blocks) {
  var allLines = [];
  for (var i = 0; i < blocks.length; i++) {
    var lines = blocks[i].split('\n');
    for (var j = 0; j < lines.length; j++) allLines.push(lines[j]);
    if (i < blocks.length - 1) allLines.push('');
  }

  var html = '';
  var idx = 0;
  while (idx < allLines.length) {
    var trimmed = (allLines[idx] || '').trim();
    if (!trimmed) { idx++; continue; }

    var headerMatch = trimmed.match(/^(#{1,4})\s+(.+)$/);
    if (headerMatch) {
      var level = headerMatch[1].length;
      html += '<h' + (level + 1) + ' class="md-heading">' + inlineMarkdown(headerMatch[2]) + '</h' + (level + 1) + '>';
      idx++;
      continue;
    }

    if (/^\*\*(.+)\*\*$/.test(trimmed)) {
      html += '<p class="md-subheading">' + inlineMarkdown(trimmed) + '</p>';
      idx++;
      continue;
    }

    if (/^\d+[\.\)]\s/.test(trimmed)) {
      var ol = collectList(allLines, idx, 'ol');
      html += ol.html;
      idx = ol.nextIdx;
      continue;
    }

    if (/^[-*]\s/.test(trimmed)) {
      var ul = collectList(allLines, idx, 'ul');
      html += ul.html;
      idx = ul.nextIdx;
      continue;
    }

    // Pipe-style markdown table: starts with `|` and contains another `|`.
    if (trimmed.charAt(0) === '|' && trimmed.indexOf('|', 1) > 0) {
      var tbl = collectTable(allLines, idx);
      if (tbl.html) {
        html += tbl.html;
        idx = tbl.nextIdx;
        continue;
      }
    }

    html += '<p>' + inlineMarkdown(trimmed) + '</p>';
    idx++;
  }
  return html;
}

// Parse a contiguous block of pipe-style markdown table rows starting at
// `startIdx`. Returns the rendered HTML and the index of the first line
// after the table. If the block isn't a valid table, returns html: ''.
function collectTable(lines, startIdx) {
  var rows = [];
  var idx = startIdx;
  while (idx < lines.length) {
    var t = (lines[idx] || '').trim();
    if (!t.startsWith('|')) break;
    // Skip separator row like `| :--- | :--- |` or `|---|---|`
    if (/^\|[\s\-:|]+\|$/.test(t)) { idx++; continue; }
    var cells = t.split('|').map(function(c) { return c.trim(); });
    if (cells[0] === '') cells.shift();
    if (cells.length && cells[cells.length - 1] === '') cells.pop();
    rows.push(cells);
    idx++;
  }

  if (rows.length === 0) return { html: '', nextIdx: startIdx };

  var html = '<div class="md-table-wrap"><table class="md-table"><thead><tr>';
  for (var h = 0; h < rows[0].length; h++) {
    html += '<th>' + inlineMarkdown(rows[0][h]) + '</th>';
  }
  html += '</tr></thead>';
  if (rows.length > 1) {
    html += '<tbody>';
    for (var r = 1; r < rows.length; r++) {
      html += '<tr>';
      for (var c = 0; c < rows[r].length; c++) {
        html += '<td>' + inlineMarkdown(rows[r][c]) + '</td>';
      }
      html += '</tr>';
    }
    html += '</tbody>';
  }
  html += '</table></div>';
  return { html: html, nextIdx: idx };
}

function collectList(lines, startIdx, type) {
  var tag = type === 'ol' ? 'ol' : 'ul';
  var mainPat = type === 'ol' ? /^\d+[\.\)]\s/ : /^[-*]\s/;
  var stripPat = type === 'ol' ? /^\d+[\.\)]\s*/ : /^[-*]\s*/;
  var html = '<' + tag + '>';
  var idx = startIdx;
  while (idx < lines.length) {
    var t = (lines[idx] || '').trim();
    if (!t) { idx++; break; }
    if (!mainPat.test(t)) break;
    var liText = t.replace(stripPat, '');
    html += '<li>' + inlineMarkdown(liText) + '</li>';
    idx++;
  }
  html += '</' + tag + '>';
  return { html: html, nextIdx: idx };
}

function inlineMarkdown(text) {
  var safe = escapeHtml(text);
  safe = safe.replace(/`([^`]+)`/g, '<code>$1</code>');
  safe = safe.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  safe = safe.replace(/__(.+?)__/g, '<strong>$1</strong>');
  safe = safe.replace(/\*(.+?)\*/g, '<em>$1</em>');
  safe = safe.replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  // Replace source citation markers with a clickable chip. Handles every
  // variant the LLM emits:
  //   [Source 1]            singular, single index
  //   [Source 1, 2, 3]      singular, comma list
  //   [Source 1,2,3]        singular, comma list (no spaces)
  //   [Source 1, Source 2]  singular, repeated prefix
  //   [Source 1-3]          singular, range
  //   [Sources 1-5]         plural, range  ← was previously dropped
  //   [Sources 1, 2, 3]     plural, comma list
  //   [Sources 1 and 2]     plural, "and" connector
  // The whole-match regex is restricted to digits / commas / hyphens /
  // whitespace / "and" plus the literal "Source(s)" prefix, so the digits
  // extracted from it are safe to interpolate without re-escaping.
  // Helper: build a clickable source chip from a sorted array of indices.
  function sourceChip(indices) {
    var clean = indices.join(',');
    var label = (indices.length > 1 ? 'Sources ' : 'Source ') + clean;
    return '<button type="button" class="source-ref" data-sources="' + clean +
      '" onclick="openSourcesSheet()" title="' + label + '" aria-label="' + label + '">' +
      '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">' +
      '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>' +
      '<path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>' +
      '</svg></button>';
  }

  // Pass 1: Full "[Source N]" / "[Sources N, M]" / "[Sources N-M]" variants.
  safe = safe.replace(
    /\[Sources?\s+\d+(?:\s*[-–]\s*\d+|\s*(?:,|and)\s*(?:Sources?\s+)?\d+)*\]/gi,
    function(match) {
      var indices = [];
      var rangeRe = /(\d+)\s*[-–]\s*(\d+)/g;
      var rangeMatch;
      while ((rangeMatch = rangeRe.exec(match)) !== null) {
        var lo = parseInt(rangeMatch[1], 10);
        var hi = parseInt(rangeMatch[2], 10);
        if (lo <= hi && hi - lo < 50) {
          for (var n = lo; n <= hi; n++) indices.push(n);
        }
      }
      var stripped = match.replace(/(\d+)\s*[-–]\s*(\d+)/g, '');
      var bare = stripped.match(/\d+/g) || [];
      bare.forEach(function(n) { indices.push(parseInt(n, 10)); });
      indices = Array.from(new Set(indices)).sort(function(a, b) { return a - b; });
      if (!indices.length) return match;
      return sourceChip(indices);
    }
  );

  // Pass 2: Bare numeric citations — [1], [2], [1][2], [1, 2], [1,2,3].
  // These appear when the LLM abbreviates "[Source N]" to just "[N]".
  // Runs AFTER pass 1 so already-converted [Source N] chips are not
  // double-matched. Only matches numbers 1-99 to avoid false positives
  // on things like [2024] (years) or [100] (HTTP status codes).
  // Adjacent [N][M] are merged into a single multi-source chip.
  safe = safe.replace(
    /(?:\[\d{1,2}(?:\s*,\s*\d{1,2})*\])+/g,
    function(match) {
      // Skip if this is inside an already-converted chip (data-sources=)
      // or inside an <a> tag / markdown link artifact.
      if (match.indexOf('data-sources') !== -1) return match;
      var nums = match.match(/\d+/g) || [];
      var indices = nums.map(function(n) { return parseInt(n, 10); })
        .filter(function(n) { return n >= 1 && n <= 99; });
      indices = Array.from(new Set(indices)).sort(function(a, b) { return a - b; });
      if (!indices.length) return match;
      return sourceChip(indices);
    }
  );

  return safe;
}

// ====== Component Renderers ======
// Map Kenjaku component types -> render functions. The server's components
// array is a tagged enum: [{type: "llm_answer"|"sources"|"suggestions"|...}, ...]
//
// Placeholder renderers for `price_list` (was comp_002) and `price_focus`
// (was comp_006) — server integration is deferred, but the slots are ready.

function renderLlmAnswer(comp) {
  var text = (comp.answer || '').trim();
  if (!text) return '';
  var paragraphs = text.split(/\n\n+/);
  return '<div class="text-content"><div class="text-body md">' +
    renderMarkdownBlocks(paragraphs) +
    '</div></div>';
}

function renderSources(comp) {
  var sources = comp.sources || [];
  if (sources.length === 0) return '';
  var count = sources.length;

  // Inject bottom sheet lazily for the actual sources list.
  setTimeout(function() { injectSourcesSheet(sources); }, 0);

  var html = '<div class="action-bar">';
  html += '<button class="action-icon" title="' + escapeHtml(t('copy')) + '" onclick="copyAnswer()">' +
    '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="13" height="13" rx="2"/>' +
    '<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg></button>';
  html += '<button class="action-icon feedback-thumb thumb-up" id="thumbUp" title="' + escapeHtml(t('helpful')) + '">' +
    '<svg viewBox="0 0 24 24"><path d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3H14zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"/></svg></button>';
  html += '<button class="action-icon feedback-thumb thumb-down" id="thumbDown" title="' + escapeHtml(t('not_helpful')) + '">' +
    '<svg viewBox="0 0 24 24"><path d="M10 15V19a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3H10zM17 2h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"/></svg></button>';
  html += '<span class="sources-pill" onclick="openSourcesSheet()">';
  html += '<svg viewBox="0 0 24 24"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>';
  html += count + ' ' + (count !== 1 ? t('source_other') : t('source_one'));
  html += '</span>';
  html += '</div>';
  return html;
}

function renderActionBarNoSources() {
  return '<div class="action-bar">' +
    '<button class="action-icon" title="' + escapeHtml(t('copy')) + '" onclick="copyAnswer()">' +
    '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="13" height="13" rx="2"/>' +
    '<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg></button>' +
    '<button class="action-icon feedback-thumb thumb-up" id="thumbUp" title="' + escapeHtml(t('helpful')) + '">' +
    '<svg viewBox="0 0 24 24"><path d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3H14zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"/></svg></button>' +
    '<button class="action-icon feedback-thumb thumb-down" id="thumbDown" title="' + escapeHtml(t('not_helpful')) + '">' +
    '<svg viewBox="0 0 24 24"><path d="M10 15V19a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3H10zM17 2h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"/></svg></button>' +
    '</div>';
}

function renderSuggestions(comp) {
  var suggestions = comp.suggestions || [];
  if (suggestions.length === 0) return '';
  var html = '<div class="related-questions">';
  for (var i = 0; i < suggestions.length; i++) {
    html += '<span class="related-q" title="' + escapeHtml(suggestions[i]) + '">' +
      '<svg class="related-icon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M12.7998 8.80005L10.7998 6.80005M12.7998 8.80005L10.7998 10.8M12.7998 8.80005L7.46647 8.80005C5.25733 8.80005 3.46647 7.00919 3.46647 4.80005" stroke="#7B849B" stroke-width="1.25" stroke-miterlimit="1.41421" stroke-linecap="round" stroke-linejoin="round"/></svg>' +
      '<span class="related-q-text">' + escapeHtml(suggestions[i]) + '</span>' +
      '</span>';
  }
  html += '</div>';
  return html;
}

// Placeholder for future server-side component. Renders a labeled empty slot
// so the layout stays stable when the server starts emitting this type.
function renderPriceList(comp) {
  var cards = (comp && comp.cards) || [];
  if (cards.length === 0) {
    return '<div class="placeholder-component">price_list · (no data)</div>';
  }
  var html = '<div class="placeholder-component"><strong>price_list</strong> · ' +
    cards.length + ' item' + (cards.length !== 1 ? 's' : '') + '</div>';
  return html;
}

function renderPriceFocus(comp) {
  var assets = (comp && comp.assets) || [];
  if (assets.length === 0) {
    return '<div class="placeholder-component">price_focus · (no data)</div>';
  }
  return '<div class="placeholder-component"><strong>price_focus</strong> · ' +
    assets.length + ' asset' + (assets.length !== 1 ? 's' : '') + '</div>';
}

// ====== Main Render ======
function renderResults(data) {
  // Capture both server-issued ids on every response. The streaming
  // path also captures them earlier in the SSE start handler — this is
  // the non-streaming path / re-confirmation.
  if (data.session_id) setSessionId(data.session_id);
  lastRequestId = data.request_id || null;
  lastQuery = (data.metadata && data.metadata.original_query) || '';
  lastResponseText = extractAnswerText(data) || '';

  var html = '';
  var components = data.components || [];

  // Group components by type so we can control layout order.
  var byType = {};
  for (var i = 0; i < components.length; i++) {
    byType[components[i].type] = components[i];
  }

  // Layout order: price_focus → price_list → llm_answer → suggestions
  // Sources become part of the action bar (below llm_answer).
  if (byType.price_focus) html += renderPriceFocus(byType.price_focus);
  if (byType.price_list)  html += renderPriceList(byType.price_list);
  if (byType.llm_answer)  html += renderLlmAnswer(byType.llm_answer);

  if (byType.sources) {
    html += renderSources(byType.sources);
  } else {
    html += renderActionBarNoSources();
  }

  if (byType.suggestions) html += renderSuggestions(byType.suggestions);

  setHtml(resultsDiv, html);
  renderDebug(data);

  // Attach suggestion click handlers
  document.querySelectorAll('.related-q').forEach(function(el) {
    el.addEventListener('click', function() {
      var q = this.textContent.trim();
      doSearch(q, true);
    });
  });

  // Attach feedback handlers
  setTimeout(function() {
    var upBtn = document.getElementById('thumbUp');
    var downBtn = document.getElementById('thumbDown');
    if (upBtn) upBtn.addEventListener('click', handleThumbUp);
    if (downBtn) downBtn.addEventListener('click', handleThumbDown);
    injectFeedbackSheet();
  }, 0);
}

function extractAnswerText(data) {
  if (!data.components) return '';
  for (var i = 0; i < data.components.length; i++) {
    if (data.components[i].type === 'llm_answer') return data.components[i].answer || '';
  }
  return '';
}

// ====== Debug Panel ======
function renderDebug(data) {
  var m = data.metadata || {};
  var tags = [];

  if (m.intent)                        tags.push('<span class="tag tag-intent">' + escapeHtml(m.intent) + '</span>');
  if (m.locale) {
    // detected_locale_source = 'llm_detected' (happy path) | 'fallback_en'
    var localeSuffix = m.detected_locale_source === 'fallback_en' ? ' (fb)' : '';
    tags.push('<span class="tag tag-lang">' + escapeHtml(String(m.locale).toUpperCase() + localeSuffix) + '</span>');
  }
  if (m.resolved_locale) {
    // resolved_locale_source: query_param | session_memory | accept_language | default | fallback
    var src = m.resolved_locale_source || '';
    var suffix = src === 'fallback' ? ' (fb)' : '';
    var label = 'resolved ' + String(m.resolved_locale).toUpperCase() + suffix +
      (src ? ' · ' + src : '');
    tags.push('<span class="tag tag-lang">' + escapeHtml(label) + '</span>');
  }
  if (m.retrieval_count !== undefined) tags.push('<span class="tag tag-tier">retrieved ' + m.retrieval_count + '</span>');
  // Web tier provenance — green pill when Brave/Serper supplied chunks,
  // separate violet pill when Gemini's built-in google_search grounding
  // metadata fired (rare on the preview model).
  if (m.grounding) {
    if (m.grounding.web_search_used) {
      var provider = m.grounding.web_search_provider || 'web';
      var count = m.grounding.web_search_count || 0;
      tags.push('<span class="tag tag-grounding-web">' +
        escapeHtml(provider) + ' · ' + count + '</span>');
    }
    if (m.grounding.gemini_grounding_used) {
      tags.push('<span class="tag tag-grounding-gemini">google_search</span>');
    }
  }
  if (m.latency_ms !== undefined)      tags.push('<span class="tag tag-time">' + m.latency_ms + 'ms</span>');
  if (m.preamble_latency_ms !== undefined) tags.push('<span class="tag tag-ttft">preamble ' + m.preamble_latency_ms + 'ms</span>');
  if (m.ttft_ms !== undefined)         tags.push('<span class="tag tag-ttft">TTFT ' + m.ttft_ms + 'ms</span>');
  if (m.llm_model)                     tags.push('<span class="tag tag-gemini">' + escapeHtml(m.llm_model) + '</span>');
  if (m.streaming)                     tags.push('<span class="tag tag-embed">streaming</span>');

  var ids = '';
  if (data.request_id) ids += '<div class="id-row"><span class="id-label">req</span><span class="id-value">' + escapeHtml(data.request_id) + '</span></div>';
  if (data.session_id) ids += '<div class="id-row"><span class="id-label">session</span><span class="id-value">' + escapeHtml(data.session_id) + '</span></div>';
  if (m.original_query)   ids += '<div class="id-row"><span class="id-label">query</span><span class="id-value">' + escapeHtml(m.original_query) + '</span></div>';
  if (m.translated_query) ids += '<div class="id-row"><span class="id-label">translated</span><span class="id-value">' + escapeHtml(m.translated_query) + '</span></div>';

  setHtml(document.getElementById('debugTags'), tags.join(''));
  setHtml(document.getElementById('debugIds'), ids);
  debugInfo.style.display = 'block';
}

// ====== Search ======
async function doSearch(query, isFollowUp) {
  if (!query.trim()) return;
  if (!isFollowUp) clearConversationState();

  showResultsView(query);
  searchInput.value = '';
  searchInput.placeholder = t('ask_followup');
  searchInput.style.height = 'auto';
  if (typeof updateCharCounter === 'function') updateCharCounter();

  if (currentAbortController) currentAbortController.abort();
  currentAbortController = new AbortController();

  try {
    // No client-generated session_id or request_id. The server creates
    // both on first contact and returns them; we capture them in the
    // SSE start handler / non-streaming response handler. Subsequent
    // calls send `X-Session-Id` automatically via getAuthHeaders().
    var reqBody = {
      query: query,
      streaming: true,
      top_k: 5,
    };

    var resp = await fetch(API_BASE + '/search', {
      method: 'POST',
      headers: getAuthHeadersWithAccept(),
      body: JSON.stringify(reqBody),
      signal: currentAbortController.signal,
    });

    if (!resp.ok) {
      hideLoading();
      var errText = await resp.text();
      setHtml(resultsDiv, '<div class="error">Error: ' + escapeHtml(errText || resp.statusText) + '</div>');
      return;
    }

    var contentType = resp.headers.get('Content-Type') || '';
    if (contentType.indexOf('text/event-stream') !== -1) {
      await handleStreamResponse(resp);
    } else {
      await handleJsonResponse(resp);
    }
  } catch (e) {
    hideLoading();
    if (e.name === 'AbortError') {
      resultsDiv.insertAdjacentHTML('beforeend', '<div class="stopped-message">You stopped this response.</div>');
    } else {
      setHtml(resultsDiv, '<div class="error">Connection error: ' + escapeHtml(e.message) + '</div>');
    }
  }
}

// Non-streaming JSON response — Kenjaku returns {success, data: SearchResponseDto}.
async function handleJsonResponse(resp) {
  var envelope = await resp.json();
  var data = envelope.data || envelope; // be permissive
  rawJsonPre.textContent = toRawJson(envelope);
  hideLoading();
  renderResults(data);
}

// SSE streaming response. Kenjaku emits three named events:
//   event: start   — StreamStartMetadata (intent, locale, retrieval_count, ...)
//   event: delta   — {text: "..."} per token
//   event: done    — StreamDoneMetadata (latency_ms, sources, suggestions, ...)
//   event: error   — {error: "..."}
async function handleStreamResponse(resp) {
  var reader = resp.body.getReader();
  var decoder = new TextDecoder();
  var buffer = '';
  var streamingText = '';
  var streamStartTs = Date.now();
  var firstDeltaTs = null;
  var startMeta = null;

  // Render a streaming slot immediately so deltas have somewhere to go.
  setHtml(resultsDiv, '<div class="text-content"><div id="streamText" class="text-body md"></div></div>');

  // Persist-across-chunks SSE state.
  var currentEvent = null;

  while (true) {
    var result = await reader.read();
    if (result.done) break;

    buffer += decoder.decode(result.value, { stream: true });
    var lines = buffer.split('\n');
    buffer = lines.pop();

    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      // End of event — blank line
      if (line === '' || line === '\r') {
        currentEvent = null;
        continue;
      }
      // Event name line
      if (line.indexOf('event:') === 0) {
        currentEvent = line.substring(6).trim();
        continue;
      }
      // Data line
      if (line.indexOf('data:') === 0) {
        var data = line.substring(5);
        if (data.charAt(0) === ' ') data = data.substring(1);
        try {
          var payload = JSON.parse(data);
          handleSseEvent(currentEvent || 'message', payload);
        } catch (e) { /* ignore malformed */ }
      }
    }
  }

  function handleSseEvent(event, payload) {
    switch (event) {
      case 'start':
        startMeta = payload;
        // Capture server-issued ids on the first contact of this tab
        // session and persist for subsequent requests + refreshes.
        if (payload.session_id) setSessionId(payload.session_id);
        if (payload.request_id) lastRequestId = payload.request_id;
        break;

      case 'delta':
        if (!firstDeltaTs) firstDeltaTs = Date.now();
        streamingText += payload.text || '';
        var el = document.getElementById('streamText');
        if (el) {
          setHtml(el, renderMarkdownBlocks(streamingText.split(/\n\n+/)));
        }
        break;

      case 'done':
        var m = startMeta || {};
        var done = payload || {};
        var fullResponse = {
          request_id: m.request_id || requestId,
          session_id: m.session_id || sessionId,
          components: buildStreamedComponents(streamingText, done.sources, done.suggestions),
          metadata: {
            original_query:   m.original_query || '',
            translated_query: m.translated_query || null,
            locale:           m.locale || '',
            detected_locale_source: m.detected_locale_source || '',
            resolved_locale:        m.resolved_locale || '',
            resolved_locale_source: m.resolved_locale_source || '',
            intent:           m.intent || 'unknown',
            retrieval_count:  m.retrieval_count || 0,
            latency_ms:       done.latency_ms || (Date.now() - streamStartTs),
            preamble_latency_ms: m.preamble_latency_ms || 0,
            ttft_ms:          firstDeltaTs ? (firstDeltaTs - streamStartTs) : null,
            llm_model:        done.llm_model || '',
            streaming:        true,
            // Web tier provenance — prefer the done event's grounding
            // (it may have been refreshed during the stream when Gemini
            // attached groundingMetadata) and fall back to start.
            grounding: done.grounding || m.grounding || null,
          },
        };
        rawJsonPre.textContent = toRawJson(fullResponse);
        hideLoading();
        renderResults(fullResponse);
        break;

      case 'error':
        hideLoading();
        setHtml(resultsDiv, '<div class="error">Stream error: ' +
          escapeHtml(payload.error || 'unknown') + '</div>');
        break;
    }
  }
}

// Assemble a SearchResponseDto-like structure from streamed state so the
// unified renderer can treat streaming and non-streaming identically.
function buildStreamedComponents(answerText, sources, suggestions) {
  var components = [];
  if (answerText) {
    components.push({ type: 'llm_answer', answer: answerText, model: 'gemini' });
  }
  if (sources && sources.length > 0) {
    components.push({ type: 'sources', sources: sources });
  }
  if (suggestions && suggestions.length > 0) {
    components.push({ type: 'suggestions', suggestions: suggestions, source: 'llm' });
  }
  return components;
}

// ====== Feedback ======
function handleThumbUp() {
  if (!lastRequestId) return;
  var current = feedbackState[lastRequestId] || null;
  if (current === 'like') {
    feedbackState[lastRequestId] = null;
    updateThumbButtons();
    submitFeedback(lastRequestId, 'cancel', null, true);
  } else {
    feedbackState[lastRequestId] = 'like';
    updateThumbButtons();
    submitFeedback(lastRequestId, 'like', null, false);
  }
}

function handleThumbDown() {
  if (!lastRequestId) return;
  var current = feedbackState[lastRequestId] || null;
  if (current === 'dislike') {
    feedbackState[lastRequestId] = null;
    updateThumbButtons();
    submitFeedback(lastRequestId, 'cancel', null, true);
  } else {
    openFeedbackSheet();
  }
}

function updateThumbButtons() {
  var current = lastRequestId ? (feedbackState[lastRequestId] || null) : null;
  var upBtn = document.getElementById('thumbUp');
  var downBtn = document.getElementById('thumbDown');
  if (upBtn) upBtn.classList.toggle('active', current === 'like');
  if (downBtn) downBtn.classList.toggle('active', current === 'dislike');
}

async function submitFeedback(requestId, action, detail, isCancel) {
  var body = { action: action };
  if (detail && detail.reason_category_id) body.reason_category_id = detail.reason_category_id;
  if (detail && detail.description)        body.description = detail.description;

  try {
    var resp = await fetch(API_BASE + '/feedback', {
      method: 'POST',
      headers: getAuthHeaders({ 'X-Request-Id': requestId }),
      body: JSON.stringify(body),
    });
    if (resp.ok) {
      if (!isCancel) showToast(t('thanks_feedback'), 'success');
    } else {
      feedbackState[requestId] = null;
      updateThumbButtons();
      showToast('Submission failed', 'error', 'Please try again');
    }
  } catch (e) {
    feedbackState[requestId] = null;
    updateThumbButtons();
    showToast('Submission failed', 'error', 'Please try again');
  }
}

// ====== Feedback Bottom Sheet ======
function injectFeedbackSheet() {
  var existing = document.getElementById('feedbackOverlay');
  if (existing) existing.remove();
  existing = document.getElementById('feedbackSheet');
  if (existing) existing.remove();

  var frame = document.querySelector('.phone-frame');
  if (!frame) return;

  var overlay = document.createElement('div');
  overlay.className = 'feedback-overlay';
  overlay.id = 'feedbackOverlay';
  overlay.onclick = closeFeedbackSheet;
  frame.appendChild(overlay);

  var sheet = document.createElement('div');
  sheet.className = 'feedback-sheet';
  sheet.id = 'feedbackSheet';

  var html = '<div class="feedback-sheet-header">';
  html += '<span class="feedback-sheet-title">' + escapeHtml(t('help_us_improve')) + '</span>';
  html += '<button class="feedback-sheet-close" id="feedbackSheetClose"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg></button>';
  html += '</div>';
  html += '<div class="feedback-reasons">';
  for (var i = 0; i < DISLIKE_REASONS.length; i++) {
    var r = DISLIKE_REASONS[i];
    html += '<label class="feedback-reason">';
    html += '<input type="radio" name="dislike_reason" value="' + r.id + '">';
    html += '<span class="feedback-radio"></span>';
    html += '<span class="feedback-reason-text">' + escapeHtml(t(r.i18n)) + '</span>';
    html += '</label>';
  }
  html += '</div>';
  html += '<textarea class="feedback-details" id="feedbackDetails" placeholder="' + escapeHtml(t('tell_us_more')) + '" rows="3"></textarea>';
  html += '<button class="feedback-submit-btn" id="feedbackSubmitBtn">' + escapeHtml(t('submit')) + '</button>';

  setHtml(sheet, html);
  frame.appendChild(sheet);

  sheet.querySelector('#feedbackSheetClose').addEventListener('click', closeFeedbackSheet);
  sheet.querySelector('#feedbackSubmitBtn').addEventListener('click', submitDislikeFeedback);
}

function openFeedbackSheet() {
  var overlay = document.getElementById('feedbackOverlay');
  var sheet = document.getElementById('feedbackSheet');
  if (overlay) overlay.classList.add('open');
  if (sheet) sheet.classList.add('open');
  var radios = sheet ? sheet.querySelectorAll('input[name="dislike_reason"]') : [];
  radios.forEach(function(r) { r.checked = false; });
  var details = document.getElementById('feedbackDetails');
  if (details) details.value = '';
}

function closeFeedbackSheet() {
  var overlay = document.getElementById('feedbackOverlay');
  var sheet = document.getElementById('feedbackSheet');
  if (overlay) overlay.classList.remove('open');
  if (sheet) sheet.classList.remove('open');
}

function submitDislikeFeedback() {
  if (!lastRequestId) return;
  var sheet = document.getElementById('feedbackSheet');
  var selected = sheet ? sheet.querySelector('input[name="dislike_reason"]:checked') : null;
  var reasonId = selected ? parseInt(selected.value, 10) : null;
  var detailsEl = document.getElementById('feedbackDetails');
  var details = detailsEl ? detailsEl.value.trim() : '';

  var detail = {};
  if (reasonId) detail.reason_category_id = reasonId;
  if (details)  detail.description = details;

  feedbackState[lastRequestId] = 'dislike';
  updateThumbButtons();
  closeFeedbackSheet();
  submitFeedback(lastRequestId, 'dislike', detail, false);
}

// ====== Sources Bottom Sheet ======
function injectSourcesSheet(sources) {
  var existing = document.getElementById('sourcesOverlay');
  if (existing) existing.remove();
  existing = document.getElementById('sourcesSheet');
  if (existing) existing.remove();

  var frame = document.querySelector('.phone-frame');
  if (!frame) return;

  var overlay = document.createElement('div');
  overlay.className = 'sources-overlay';
  overlay.id = 'sourcesOverlay';
  overlay.onclick = closeSourcesSheet;
  frame.appendChild(overlay);

  var sheet = document.createElement('div');
  sheet.className = 'sources-sheet';
  sheet.id = 'sourcesSheet';

  var html = '<div class="sources-sheet-header">';
  html += '<span class="sources-sheet-title">' + escapeHtml(t('sources_title')) + '</span>';
  html += '<button class="sources-sheet-close" onclick="closeSourcesSheet()"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg></button>';
  html += '</div>';

  var ul = document.createElement('ul');
  ul.className = 'sources-list';
  for (var i = 0; i < sources.length; i++) {
    var src = sources[i];
    var title = src.title || src.name || src.url || 'Source';
    var url = src.url || '';
    var domain = '';
    try { domain = new URL(url).hostname; } catch (e) { domain = url; }

    var li = document.createElement('li');
    li.className = 'source-item';
    li.setAttribute('data-source-url', url);
    setHtml(li, '<span class="source-number">' + (i + 1) + '</span>' +
      '<div class="source-content">' +
      '<span class="source-title"></span>' +
      '<span class="source-domain"><span class="source-domain-text"></span></span>' +
      '</div>');
    li.querySelector('.source-title').textContent = title;
    li.querySelector('.source-domain-text').textContent = domain;
    li.addEventListener('click', function() {
      var u = this.getAttribute('data-source-url');
      if (u) window.open(u, '_blank');
    });
    ul.appendChild(li);
  }
  setHtml(sheet, html);
  sheet.appendChild(ul);
  frame.appendChild(sheet);
}

function openSourcesSheet() {
  var overlay = document.getElementById('sourcesOverlay');
  var sheet = document.getElementById('sourcesSheet');
  if (overlay) overlay.classList.add('open');
  if (sheet) sheet.classList.add('open');
}

function closeSourcesSheet() {
  var overlay = document.getElementById('sourcesOverlay');
  var sheet = document.getElementById('sourcesSheet');
  if (overlay) overlay.classList.remove('open');
  if (sheet) sheet.classList.remove('open');
}

// ====== Copy / Toast ======
function copyAnswer() {
  var el = document.querySelector('.text-content');
  if (el) {
    navigator.clipboard.writeText(el.innerText).then(function() {
      showToast('Response copied', 'success');
    }).catch(function() {
      showToast('Copy failed', 'error');
    });
  }
}

function showToast(message, type, subtitle) {
  var existing = document.querySelector('.feedback-toast');
  if (existing) existing.remove();
  var frame = document.querySelector('.phone-frame');
  if (!frame) return;

  var toast = document.createElement('div');
  toast.className = 'feedback-toast ' + (type || 'success');
  var iconSvg = type === 'error'
    ? '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M18 6L6 18M6 6l12 12"/></svg>'
    : '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg>';

  var html = '<span class="toast-icon">' + iconSvg + '</span>';
  html += '<span class="toast-body"><span class="toast-title">' + escapeHtml(message) + '</span>';
  if (subtitle) html += '<span class="toast-subtitle">' + escapeHtml(subtitle) + '</span>';
  html += '</span>';
  setHtml(toast, html);
  frame.appendChild(toast);

  requestAnimationFrame(function() { toast.classList.add('show'); });
  setTimeout(function() {
    toast.classList.remove('show');
    setTimeout(function() { toast.remove(); }, 300);
  }, 2500);
}

// ====== Auto-complete ======
var acDropdown = document.getElementById('autoCompleteDropdown');
var acActiveIndex = -1;
var acSuggestions = [];
var acAbortController = null;

function debounce(fn, delay) {
  var timer = null;
  return function() {
    var that = this;
    var args = arguments;
    if (timer) clearTimeout(timer);
    timer = setTimeout(function() { fn.apply(that, args); }, delay);
  };
}

function acShow() { acDropdown.classList.add('visible'); searchInput.setAttribute('aria-expanded', 'true'); }
function acHide() {
  acDropdown.classList.remove('visible');
  searchInput.setAttribute('aria-expanded', 'false');
  searchInput.removeAttribute('aria-activedescendant');
  acActiveIndex = -1;
  acSuggestions = [];
  while (acDropdown.firstChild) acDropdown.removeChild(acDropdown.firstChild);
}

function acHighlight(suggestion, query) {
  var lower = suggestion.toLowerCase();
  var qLower = query.toLowerCase();
  var idx = lower.indexOf(qLower);
  if (idx === -1) return '<span class="ac-rest">' + escapeHtml(suggestion) + '</span>';
  var before = suggestion.slice(0, idx);
  var match = suggestion.slice(idx, idx + query.length);
  var after = suggestion.slice(idx + query.length);
  var html = '';
  if (before) html += '<span class="ac-rest">' + escapeHtml(before) + '</span>';
  html += '<span class="ac-match">' + escapeHtml(match) + '</span>';
  if (after) html += '<span class="ac-rest">' + escapeHtml(after) + '</span>';
  return html;
}

function acRender(suggestions, query) {
  while (acDropdown.firstChild) acDropdown.removeChild(acDropdown.firstChild);
  acSuggestions = suggestions;
  acActiveIndex = -1;
  if (!suggestions.length) { acHide(); return; }
  var searchIcon = '<svg class="ac-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/></svg>';
  for (var i = 0; i < suggestions.length; i++) {
    var item = document.createElement('div');
    item.className = 'ac-item';
    item.id = 'ac-option-' + i;
    item.setAttribute('role', 'option');
    item.dataset.index = i;
    setHtml(item, searchIcon + '<span>' + acHighlight(suggestions[i], query) + '</span>');
    acDropdown.appendChild(item);
  }
  acShow();
}

function acSetActive(index) {
  var items = acDropdown.querySelectorAll('.ac-item');
  for (var i = 0; i < items.length; i++) items[i].classList.remove('active');
  acActiveIndex = index;
  if (index >= 0 && index < items.length) {
    items[index].classList.add('active');
    searchInput.setAttribute('aria-activedescendant', 'ac-option-' + index);
  } else {
    searchInput.removeAttribute('aria-activedescendant');
  }
}

function acSelect(index) {
  if (index >= 0 && index < acSuggestions.length) {
    var text = acSuggestions[index];
    searchInput.value = text;
    acHide();
    var isFollowUp = resultsView.style.display !== 'none';
    doSearch(text, isFollowUp);
  }
}

var acFetchSuggestions = debounce(function() {
  var query = searchInput.value.trim();
  if (query.length < 3) { acHide(); return; }
  if (acAbortController) acAbortController.abort();
  acAbortController = new AbortController();

  var url = API_BASE + '/autocomplete?q=' + encodeURIComponent(query) +
    '&limit=5' + localeQueryString();

  fetch(url, { headers: getAuthHeaders(), signal: acAbortController.signal })
    .then(function(resp) { return resp.ok ? resp.json() : null; })
    .then(function(envelope) {
      if (!envelope) { acHide(); return; }
      var data = envelope.data || envelope;
      var current = searchInput.value.trim();
      if (current.length < 3) { acHide(); return; }
      acRender(data.suggestions || [], current);
    })
    .catch(function(err) { if (err.name !== 'AbortError') acHide(); });
}, 300);

searchInput.addEventListener('input', acFetchSuggestions);
acDropdown.addEventListener('click', function(e) {
  var item = e.target.closest('.ac-item');
  if (item) acSelect(parseInt(item.dataset.index, 10));
});
document.addEventListener('click', function(e) {
  if (!acDropdown.contains(e.target) && e.target !== searchInput) acHide();
});
searchInput.addEventListener('blur', function() {
  setTimeout(function() { acHide(); }, 150);
});

// ====== Top Searches (pills) ======
var PILL_COUNT = 6;
async function loadPills() {
  try {
    var url = API_BASE + '/top-searches?limit=' + PILL_COUNT +
      '&period=daily' + localeQueryString();
    var resp = await fetch(url, { headers: getAuthHeaders() });
    if (!resp.ok) return;
    var envelope = await resp.json();
    var data = envelope.data || envelope || {};
    // New shape: { items: BlendedItemDto[], resolved_locale, resolved_locale_source }.
    // Legacy shape (pre default-suggestions-locale): a bare array.
    var items = Array.isArray(data) ? data : (data.items || []);
    while (pillsDiv.firstChild) pillsDiv.removeChild(pillsDiv.firstChild);
    for (var i = 0; i < items.length && i < PILL_COUNT; i++) {
      var it = items[i];
      var text = typeof it === 'string' ? it : (it.query || it.text || '');
      if (!text) continue;
      var btn = document.createElement('button');
      btn.dataset.query = text;
      btn.textContent = text;
      pillsDiv.appendChild(btn);
    }
  } catch (e) { /* non-critical */ }
}

pillsDiv.addEventListener('click', function(e) {
  if (e.target.tagName === 'BUTTON' && e.target.dataset.query) {
    doSearch(e.target.dataset.query, false);
  }
});

// ====== Event Handlers ======
searchBtn.addEventListener('click', function() {
  if (currentAbortController) { abortCurrentSearch(); return; }
  submitFromInput();
});

// ====== Input composition (IME) + auto-grow + char counter ======
// imeComposing tracks whether an IME session is in progress (CJK, etc.).
// While true, Enter MUST NOT submit — the user is still selecting
// candidates. We also check `e.isComposing` on the keydown event as a
// belt-and-braces guard, since some browsers still fire a keydown with
// keyCode 229 before compositionend.
var imeComposing = false;
searchInput.addEventListener('compositionstart', function() { imeComposing = true; });
searchInput.addEventListener('compositionend',   function() { imeComposing = false; });

var MAX_INPUT_CHARS = 500;
var charCounter = document.getElementById('charCounter');
function updateCharCounter() {
  if (!charCounter) return;
  var len = searchInput.value.length;
  charCounter.textContent = len + ' / ' + MAX_INPUT_CHARS;
  charCounter.classList.toggle('near-limit', len >= MAX_INPUT_CHARS * 0.9 && len < MAX_INPUT_CHARS);
  charCounter.classList.toggle('at-limit',   len >= MAX_INPUT_CHARS);
}

// Auto-grow textarea between 1 and 3 lines. CSS caps height at 60px so
// anything beyond spills into the scrollbar.
function autoGrowInput() {
  searchInput.style.height = 'auto';
  var newH = Math.min(searchInput.scrollHeight, 60);
  searchInput.style.height = newH + 'px';
}

searchInput.addEventListener('input', function() {
  updateCharCounter();
  autoGrowInput();
});

function submitFromInput() {
  var val = searchInput.value.trim();
  if (!val) return;
  acHide();
  var isFollowUp = resultsView.style.display !== 'none';
  doSearch(val, isFollowUp);
}

searchInput.addEventListener('keydown', function(e) {
  // Never intercept keys while an IME is composing. The `229` keyCode
  // fallback catches browsers that set `isComposing` to false but still
  // route the keydown through the IME.
  if (imeComposing || e.isComposing || e.keyCode === 229) return;

  if (acDropdown.classList.contains('visible')) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      var next = acActiveIndex + 1;
      if (next >= acSuggestions.length) next = 0;
      acSetActive(next);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      var prev = acActiveIndex - 1;
      if (prev < 0) prev = acSuggestions.length - 1;
      acSetActive(prev);
      return;
    }
    if (e.key === 'Escape') { e.preventDefault(); acHide(); return; }
    if (e.key === 'Enter' && !e.shiftKey && acActiveIndex >= 0) {
      e.preventDefault();
      acSelect(acActiveIndex);
      return;
    }
  }
  if (e.key === 'Enter') {
    // Shift+Enter inserts a newline — let the default textarea behavior
    // handle it, then resize.
    if (e.shiftKey) {
      setTimeout(autoGrowInput, 0);
      return;
    }
    e.preventDefault();
    submitFromInput();
  }
});

// Initial counter state
updateCharCounter();

document.getElementById('backBtn').addEventListener('click', function() {
  showSearchView();
  clearConversationState();
});

// Boot
loadPills();
