// Run synchronously before <body> renders so the page never flashes the
// wrong theme on first paint. We read the stored preference (or fall
// back to the OS-level prefers-color-scheme) and stamp the data-theme
// attribute onto <html> immediately.
(function () {
    var stored = null;
    try { stored = localStorage.getItem("ruzit-theme"); } catch (_) { stored = null; }
    var prefDark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    var theme = stored || (prefDark ? "dark" : "light");
    document.documentElement.setAttribute("data-theme", theme);
})();

// Wire the toggle button after the sidebar partial is injected. The
// sidebar loader sets a `ruzit:sidebar-ready` event when done; we
// listen for it. Falls back to delegated click on the document for
// robustness if the event ever stops firing.
function wireThemeToggle() {
    var btn = document.querySelector(".theme-toggle");
    if (!btn || btn.dataset.bound === "1") return;
    btn.dataset.bound = "1";
    btn.addEventListener("click", function () {
        var current = document.documentElement.getAttribute("data-theme") || "light";
        var next = current === "dark" ? "light" : "dark";
        document.documentElement.setAttribute("data-theme", next);
        try { localStorage.setItem("ruzit-theme", next); } catch (_) { }
    });
}

document.addEventListener("ruzit:sidebar-ready", wireThemeToggle);
document.addEventListener("DOMContentLoaded", wireThemeToggle);
