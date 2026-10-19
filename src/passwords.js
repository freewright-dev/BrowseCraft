/* BrowseCraft password manager: page script.
 *
 * Runs in the top document of every web page. It
 *   1. finds login, sign-up and change-password forms and tells the browser
 *      which kind the page has (`bc:forms?kind=…`);
 *   2. reports the credentials a form is submitted with (`bc:cred?…`) so the
 *      browser can offer to save them;
 *   3. defines `window.__bcFill(user, pass)`, which the browser calls (only
 *      when the user picked a login) to fill the page.
 * It never reads or stores anything itself. */
(function () {
  'use strict';
  var top = true;
  try { top = window.top === window; } catch (e) { top = false; }
  if (!top || window.__bcPasswords) return;
  try {
    Object.defineProperty(window, '__bcPasswords', { value: true });
  } catch (e) {}

  var send = function (path, params) {
    try {
      window.ipc.postMessage('bc:' + path + '?' + new URLSearchParams(params).toString());
    } catch (e) {}
  };

  var visible = function (el) {
    try {
      var r = el.getBoundingClientRect();
      var s = getComputedStyle(el);
      return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
    } catch (e) { return false; }
  };

  var passwordFields = function (scope) {
    return Array.prototype.filter.call(
      (scope || document).querySelectorAll('input[type=password]'),
      function (el) { return !el.disabled && visible(el); });
  };

  var scopeOf = function (pw) {
    return pw.form || pw.closest('form') || pw.closest('[role=dialog],main,section,div') || document;
  };

  var TEXT_TYPES = /^(text|email|tel|)$/i;

  /* The username field of a password field: the nearest visible text-like
     input before it, or one named like a username. */
  var usernameField = function (pw) {
    var scope = scopeOf(pw);
    var fields = Array.prototype.filter.call(scope.querySelectorAll('input'), function (i) {
      return !i.disabled && TEXT_TYPES.test(i.getAttribute('type') || '') && visible(i);
    });
    var before = null;
    fields.forEach(function (f) {
      if (f.compareDocumentPosition(pw) & Node.DOCUMENT_POSITION_FOLLOWING) before = f;
    });
    var named = fields.filter(function (f) {
      return /user|login|e-?mail|account|identifier/i.test(
        (f.name || '') + ' ' + (f.id || '') + ' ' + (f.getAttribute('autocomplete') || ''));
    })[0];
    return before || named || null;
  };

  var SIGNUP_WORDS = /sign.?up|register|registration|create.{0,12}account|new.account|join|get.started/i;
  var CHANGE_WORDS = /change.{0,12}password|new.password|reset.password/i;

  /* login, signup or change-password */
  var classify = function (pw) {
    var scope = scopeOf(pw);
    var fields = passwordFields(scope);
    var ac = function (f) { return (f.getAttribute('autocomplete') || '').toLowerCase(); };
    var hasNew = fields.some(function (f) { return ac(f) === 'new-password'; });
    var hasCurrent = fields.some(function (f) { return ac(f) === 'current-password'; });
    if (hasNew && hasCurrent) return 'change';
    if (hasNew) return 'signup';
    var label = [scope.id, scope.name, scope.action, scope.getAttribute && scope.getAttribute('class')]
      .join(' ');
    var buttons = Array.prototype.map.call(
      scope.querySelectorAll('button,input[type=submit],h1,h2,legend'),
      function (b) { return (b.innerText || b.textContent || b.value || '').slice(0, 60); }).join(' ');
    var words = label + ' ' + buttons;
    if (fields.length >= 3) return 'change';
    if (fields.length === 2 || SIGNUP_WORDS.test(words)) {
      return CHANGE_WORDS.test(words) && !SIGNUP_WORDS.test(words) ? 'change' : 'signup';
    }
    if (CHANGE_WORDS.test(words)) return 'change';
    return 'login';
  };

  /* ---- report which kind of form the page has ---- */
  var lastKind = '';
  var scan = function () {
    var pws = passwordFields();
    var kind = pws.length ? classify(pws[0]) : '';
    if (kind !== lastKind) {
      lastKind = kind;
      send('forms', { kind: kind || 'none' });
    }
  };
  var timer = 0;
  var rescan = function () {
    clearTimeout(timer);
    timer = setTimeout(scan, 350);
  };
  try {
    new MutationObserver(rescan).observe(document, { childList: true, subtree: true });
  } catch (e) {}
  document.addEventListener('DOMContentLoaded', rescan);
  window.addEventListener('load', rescan);
  rescan();

  /* ---- report submitted credentials ---- */
  var lastSent = '';
  var lastSentAt = 0;
  var capture = function (scope) {
    try {
      var pws = passwordFields(scope).filter(function (f) { return f.value; });
      if (!pws.length) return;
      var kind = classify(pws[0]);
      var fresh = pws.filter(function (f) {
        return (f.getAttribute('autocomplete') || '').toLowerCase() === 'new-password';
      })[0];
      var chosen;
      if (kind === 'login') chosen = pws[0];
      else if (kind === 'change') chosen = fresh || pws[pws.length > 1 ? 1 : 0];
      else chosen = fresh || pws[0];
      if (kind === 'signup' && pws.length > 1) {
        /* A new password typed twice is only trusted when both match. */
        var same = pws.some(function (f) { return f !== chosen && f.value === chosen.value; });
        if (!same) return;
      }
      var userField = usernameField(chosen);
      var user = userField ? userField.value : '';
      var pass = chosen.value;
      if (user.length > 256 || pass.length > 1024) return;
      var key = kind + '\u0000' + user + '\u0000' + pass;
      var now = Date.now();
      if (key === lastSent && now - lastSentAt < 2500) return;
      lastSent = key;
      lastSentAt = now;
      send('cred', { k: kind, u: user, p: pass, url: location.href });
    } catch (e) {}
  };

  document.addEventListener('submit', function (e) {
    capture(e.target);
  }, true);
  document.addEventListener('click', function (e) {
    var t = e.target && e.target.closest ? e.target.closest('button,input[type=submit],input[type=button],[role=button]') : null;
    if (!t) return;
    var scope = t.form || t.closest('form') || t.closest('[role=dialog],main,section,div');
    if (scope && passwordFields(scope).length) capture(scope);
  }, true);
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || !e.target || e.target.tagName !== 'INPUT') return;
    var scope = e.target.form || e.target.closest('form') || e.target.closest('[role=dialog],main,section,div');
    if (scope && passwordFields(scope).length) capture(scope);
  }, true);

  /* ---- fill (called by the browser after the user picked a login) ---- */
  var setValue = function (el, value) {
    var proto = Object.getPrototypeOf(el);
    var desc = Object.getOwnPropertyDescriptor(proto, 'value');
    if (desc && desc.set) desc.set.call(el, value); else el.value = value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };
  var fill = function (user, pass) {
    var pws = passwordFields();
    var filled = 0;
    if (pws.length) {
      if (user !== null && user !== undefined) {
        var uf = usernameField(pws[0]);
        if (uf) { setValue(uf, String(user)); filled++; }
      }
      if (pass !== null && pass !== undefined) {
        pws.forEach(function (f) { setValue(f, String(pass)); filled++; });
      }
    } else if (user !== null && user !== undefined) {
      /* A first step that only asks for the user name. */
      var singles = Array.prototype.filter.call(document.querySelectorAll('input'), function (i) {
        return !i.disabled && TEXT_TYPES.test(i.getAttribute('type') || '') && visible(i);
      });
      if (singles.length === 1) { setValue(singles[0], String(user)); filled++; }
    }
    return filled;
  };
  try {
    Object.defineProperty(window, '__bcFill', { value: fill });
  } catch (e) {}
})();
