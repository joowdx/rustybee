// rustybee UI helpers: segment counter, recipient chips, copy buttons, countdowns, filter rule rows, live refresh.
(function () {
  "use strict";
  var BASIC = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞ\u001bÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
  var EXT = "\f^{}\\[~]|€";

  // Same algorithm as domain::sms::segments.
  function segments(body) {
    var septets = 0, gsm = true;
    for (var ch of body) {
      if (BASIC.indexOf(ch) >= 0) septets += 1;
      else if (EXT.indexOf(ch) >= 0) septets += 2;
      else { gsm = false; break; }
    }
    var units = gsm ? septets : body.length;
    var single = gsm ? 160 : 70, multi = gsm ? 153 : 67, parts, remaining;
    if (units === 0) { parts = 0; remaining = single; }
    else if (units <= single) { parts = 1; remaining = single - units; }
    else { parts = Math.ceil(units / multi); remaining = parts * multi - units; }
    return { encoding: gsm ? "GSM-7" : "UCS-2", chars: [...body].length, parts: parts, remaining: remaining };
  }

  var PHONE = /^\+?[0-9]{3,20}$/;

  function init(root) {
    root.querySelectorAll("textarea[data-counter]").forEach(function (ta) {
      if (ta.dataset.ready) return;
      ta.dataset.ready = "1";
      var out = document.getElementById(ta.dataset.counter);
      var update = function () {
        var s = segments(ta.value);
        if (out) out.textContent = s.encoding + " · " + s.chars + " chars · " + s.parts + " part" + (s.parts === 1 ? "" : "s") + " · " + s.remaining + " left";
      };
      ta.addEventListener("input", update);
      update();
    });
    root.querySelectorAll("[data-chips]").forEach(function (input) {
      if (input.dataset.ready) return;
      input.dataset.ready = "1";
      var box = document.getElementById(input.dataset.chips);
      var update = function () {
        if (!box) return;
        box.textContent = "";
        input.value.split(/[\n,;]+/).map(function (v) { return v.trim(); }).filter(Boolean).forEach(function (v) {
          var span = document.createElement("span");
          span.className = "chip" + (PHONE.test(v.replace(/[\s\-().]/g, "")) ? "" : " bad");
          span.textContent = v;
          box.appendChild(span);
        });
      };
      input.addEventListener("input", update);
      update();
    });
    root.querySelectorAll("[data-copy]").forEach(function (btn) {
      if (btn.dataset.ready) return;
      btn.dataset.ready = "1";
      btn.addEventListener("click", function (e) {
        e.preventDefault();
        var el = document.getElementById(btn.dataset.copy);
        if (!el || !navigator.clipboard) return;
        navigator.clipboard.writeText(el.textContent.trim()).then(function () {
          var t = btn.textContent;
          btn.textContent = "Copied";
          setTimeout(function () { btn.textContent = t; }, 1200);
        });
      });
    });
    root.querySelectorAll("[data-expires]").forEach(function (el) {
      if (el.dataset.ready) return;
      el.dataset.ready = "1";
      var end = parseInt(el.dataset.expires, 10);
      var tick = function () {
        var s = Math.max(0, Math.round((end - Date.now()) / 1000));
        el.textContent = s > 0 ? "expires in " + Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0") : "expired";
        if (s > 0 && document.body.contains(el)) setTimeout(tick, 1000);
      };
      tick();
    });
    root.querySelectorAll("[data-forwarding]").forEach(function (sw) {
      if (sw.dataset.ready) return;
      sw.dataset.ready = "1";
      var group = sw.form && sw.form.querySelector("[data-forwarding-filters]");
      if (group) sw.addEventListener("change", function () { group.disabled = !sw.checked; });
    });
    root.querySelectorAll("[data-rules]").forEach(function (box) {
      if (box.dataset.ready) return;
      box.dataset.ready = "1";
      var form = box.closest("form");
      var mode = form && form.querySelector("[data-filter-mode]");
      var list = box.querySelector("[data-rule-list]");
      var tpl = box.querySelector("[data-rule-template]");
      var add = box.querySelector("[data-rule-add]");
      var warn = box.querySelector("[data-rules-warn]");
      var note = box.querySelector("[data-rules-note]");
      var used = function () {
        return [].filter.call(list.querySelectorAll("input[name=rule_value]"), function (i) { return i.value.trim() !== ""; }).length;
      };
      var update = function () {
        var m = mode ? mode.value : "off";
        box.classList.toggle("off", m === "off");
        if (warn) warn.hidden = !(m === "allow" && used() === 0);
        if (note) note.textContent = m === "off"
          ? "Kept, but not used while everything is forwarded."
          : "A text matches when any rule matches. Rows left blank are dropped.";
      };
      var wire = function (row) {
        var rm = row.querySelector("[data-rule-remove]");
        if (rm) {
          rm.hidden = false;
          rm.addEventListener("click", function () {
            var next = row.nextElementSibling || row.previousElementSibling;
            row.remove();
            if (!list.children.length) addRow();
            update();
            var f = next && next.querySelector("input");
            if (f) f.focus();
          });
        }
        row.querySelector("input").addEventListener("input", update);
      };
      var addRow = function () {
        var row = tpl.content.firstElementChild.cloneNode(true);
        list.appendChild(row);
        wire(row);
        return row;
      };
      // With JavaScript, rows are added on demand: drop the spare blank row unless it is the only one.
      var rows = list.children;
      if (rows.length > 1) {
        var last = rows[rows.length - 1];
        if (last.querySelector("input").value === "") last.remove();
      }
      [].forEach.call(list.children, wire);
      if (add) {
        add.hidden = false;
        add.addEventListener("click", function () { addRow().querySelector("input").focus(); });
      }
      if (mode) mode.addEventListener("change", update);
      update();
    });
  }

  // Close the mobile "More" menu on an outside tap or Escape.
  document.addEventListener("click", function (e) {
    document.querySelectorAll("details.more[open],details.pop[open]").forEach(function (d) { if (!d.contains(e.target)) d.open = false; });
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") document.querySelectorAll("details.more[open],details.pop[open]").forEach(function (d) { d.open = false; d.querySelector("summary").focus(); });
  });

  // Live dashboard: refresh at most once a second, and always once after the last event of a burst.
  // htmx's own throttle drops events inside the window, which left a card on a stale state.
  var liveTimer = null, livePending = false;
  function refreshLive() {
    var live = document.getElementById("live");
    if (!live) return;
    htmx.trigger(live, "refresh-live");
    liveTimer = setTimeout(function () {
      liveTimer = null;
      if (livePending) { livePending = false; refreshLive(); }
    }, 1000);
  }
  document.addEventListener("htmx:sseMessage", function (e) {
    if (!e.target.dataset || e.target.dataset.refresh !== "live") return;
    if (liveTimer) livePending = true;
    else refreshLive();
  });

  // Turnstile data-callback on the login page: enable the gated buttons.
  window.rbTurnstileOk = function () {
    document.querySelectorAll("button[data-ts]").forEach(function (b) { b.disabled = false; });
  };

  document.addEventListener("DOMContentLoaded", function () { init(document); });
  document.addEventListener("htmx:load", function (e) { init(e.target); });
})();
