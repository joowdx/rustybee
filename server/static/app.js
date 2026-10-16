// rustybee UI helpers: segment counter, recipient chips, copy buttons, countdowns.
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
  }

  document.addEventListener("DOMContentLoaded", function () { init(document); });
  document.addEventListener("htmx:load", function (e) { init(e.target); });
})();
