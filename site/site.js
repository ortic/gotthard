// The main button downloads the installer on Windows, and the Windows card
// is marked. build.py writes the newest release's links into the button's
// data attributes; without JavaScript it leads to #download.
(() => {
  "use strict";

  function onWindows() {
    const ua = navigator.userAgent || "";
    const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    if (/Android|CrOS|iPhone|iPad/i.test(ua)) return false;
    return /Win/i.test(platform) || /Windows/i.test(ua);
  }

  function main() {
    if (!onWindows()) return;
    const card = document.querySelector('.platform[data-os="windows"]');
    if (card) card.classList.add("is-yours");

    const button = document.getElementById("hero-download");
    const note = document.getElementById("hero-note");
    if (!button || !button.dataset.windows) return;
    button.href = button.dataset.windows;
    button.textContent = "Download for Windows";
    if (note) {
      const a = document.createElement("a");
      a.href = "#download";
      a.textContent = "Other downloads";
      note.replaceChildren(
        `Installer (.exe) · ${button.dataset.windowsSize} · Version ${note.dataset.version || ""} · `,
        a,
      );
    }
  }

  // A link to a question of the FAQ opens it.
  function openTarget() {
    const el = location.hash && document.getElementById(location.hash.slice(1));
    if (el && el.tagName === "DETAILS") el.open = true;
  }

  addEventListener("hashchange", openTarget);
  openTarget();
  main();
})();
