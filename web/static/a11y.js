// Shared accessibility helpers, loaded by every page before its own script.
// Exposes window.a11y = { announce, openDialog }.
(function () {
  'use strict';

  // --- polite live-region announcer -------------------------------------
  // A single visually-hidden region per page. Async results (build done,
  // readiness verdict, save status, sweep progress) call announce() so screen
  // readers hear them; sighted users are unaffected.
  let region = null;
  function ensureRegion() {
    if (region) return region;
    region = document.createElement('div');
    region.setAttribute('aria-live', 'polite');
    region.setAttribute('aria-atomic', 'true');
    region.className = 'sr-only';
    document.body.appendChild(region);
    return region;
  }
  function announce(msg) {
    const r = ensureRegion();
    r.textContent = '';
    // re-set after a tick so repeated identical text still announces. setTimeout
    // (not requestAnimationFrame) so it still fires when the tab is backgrounded
    // during a long build/validate.
    setTimeout(() => { r.textContent = msg == null ? '' : String(msg); }, 50);
  }

  // --- accessible modal dialog ------------------------------------------
  // Marks the overlay as a dialog, moves focus inside, traps Tab within it,
  // and restores focus to the trigger on close. Returns a closer to call from
  // the page's own close handler. `opts.focus` = selector to focus first.
  const FOCUSABLE =
    'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

  function visibleFocusables(root) {
    return Array.prototype.filter.call(
      root.querySelectorAll(FOCUSABLE),
      (el) => el.offsetParent !== null || el === document.activeElement
    );
  }

  let dlgSeq = 0;
  function openDialog(overlay, opts) {
    opts = opts || {};
    const prev = document.activeElement;
    if (!overlay.getAttribute('role')) overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');

    const card = overlay.querySelector('[data-dialog-card]') || overlay.firstElementChild || overlay;
    // name the dialog from its first heading, if it isn't already labelled
    if (!overlay.getAttribute('aria-label') && !overlay.getAttribute('aria-labelledby')) {
      const h = card.querySelector('h1,h2,h3,h4');
      if (h) {
        if (!h.id) h.id = 'dlg-title-' + (++dlgSeq);
        overlay.setAttribute('aria-labelledby', h.id);
      }
    }
    const first =
      (opts.focus && overlay.querySelector(opts.focus)) ||
      visibleFocusables(card)[0] ||
      card;
    // an info card with no controls still needs to receive focus so Tab is trapped
    if (first === card && !card.hasAttribute('tabindex')) card.tabIndex = -1;
    if (first && first.focus) first.focus();

    function onKey(e) {
      if (e.key !== 'Tab') return;
      const items = visibleFocusables(overlay);
      if (!items.length) return;
      const a = items[0];
      const b = items[items.length - 1];
      if (e.shiftKey && document.activeElement === a) { e.preventDefault(); b.focus(); }
      else if (!e.shiftKey && document.activeElement === b) { e.preventDefault(); a.focus(); }
    }
    overlay.addEventListener('keydown', onKey);

    return function closeDialog() {
      overlay.removeEventListener('keydown', onKey);
      overlay.removeAttribute('aria-modal');
      if (prev && prev.focus) prev.focus();
    };
  }

  window.a11y = { announce, openDialog };
})();
