// Tiny sidebar loader. Each page emits an empty <aside class="sidebar"></aside>
// and includes this script; we fetch _sidebar.html, inject it, then mark the
// link matching the current URL as active. One source of truth means new
// tabs only need a one-line edit to _sidebar.html instead of a 17-page sweep.
(async () => {
    const host = document.querySelector("aside.sidebar");
    if (!host) return;
    let html;
    try {
        html = await fetch("_sidebar.html", { cache: "no-cache" }).then(r => r.text());
    } catch (_) {
        return;
    }
    host.innerHTML = html;

    // Mark the active link. We compare against the bare filename so anchor
    // suffixes and trailing slashes don't trip the match. The default page
    // ("/" → index.html) is handled explicitly.
    const here = (location.pathname.split("/").pop() || "index.html").toLowerCase();
    host.querySelectorAll("nav a").forEach(a => {
        const href = (a.getAttribute("href") || "").split("#")[0].toLowerCase();
        if (!href) return;
        if (href === here) a.classList.add("active");
    });
})();
