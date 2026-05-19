(() => {
    "use strict";

    const main = document.querySelector("main");
    const rail = document.querySelector("aside.right-toc");
    if (!main || !rail) return;

    function slugify(text) {
        return text
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, "-")
            .replace(/^-+|-+$/g, "")
            .slice(0, 80);
    }

    function classifyMember(member) {
        if (member.dataset.kind) return member.dataset.kind;
        const name = (member.querySelector(".member-name")?.textContent || "").trim();
        const hasMethodTag = !!member.querySelector(".tag-method");
        const hasReadonly = !!member.querySelector(".tag-readonly");
        const looksLikeEvent = /^\.[A-Z][a-zA-Z0-9]*\s*$/.test(name) &&
            /Signal/i.test(member.textContent || "");
        let kind = "property";
        if (hasMethodTag || /[( ]?\(/.test(name) || name.startsWith(":")) {
            kind = "method";
        }
        if (looksLikeEvent) kind = "event";
        if (hasReadonly && kind === "method") kind = "property";
        member.dataset.kind = kind;
        return kind;
    }

    function isDeprecated(member) {
        return !!member.querySelector(".tag-deprecated, .deprecated-tag");
    }

    function ensureId(el, fallback) {
        if (el.id) return el.id;
        const txt = (el.textContent || fallback).trim();
        const slug = slugify(txt);
        if (!slug) return "";
        let unique = slug;
        let n = 2;
        while (document.getElementById(unique)) {
            unique = `${slug}-${n++}`;
        }
        el.id = unique;
        return unique;
    }

    const sections = [];
    let current = null;

    Array.from(main.children).forEach((el) => {
        const tag = el.tagName;
        if (tag === "H2") {
            ensureId(el, "section");
            current = { heading: el, members: [] };
            sections.push(current);
        } else if (current && el.classList && el.classList.contains("member-list")) {
            el.querySelectorAll(".member").forEach((member, idx) => {
                const nameEl = member.querySelector(".member-name");
                const display = (nameEl?.textContent || `item ${idx + 1}`).trim();
                const fallback = `${current.heading.id}-${slugify(display) || idx + 1}`;
                ensureId(member, fallback);
                classifyMember(member);
                current.members.push({ el: member, display });
            });
        }
    });

    if (sections.length === 0) return;

    const heading = document.createElement("p");
    heading.className = "right-toc-heading";
    heading.textContent = "On this page";
    rail.appendChild(heading);

    const nav = document.createElement("nav");
    rail.appendChild(nav);

    sections.forEach((sec) => {
        const a = document.createElement("a");
        a.className = "right-toc-section";
        a.href = `#${sec.heading.id}`;
        a.textContent = sec.heading.textContent.trim();
        nav.appendChild(a);

        sec.members.forEach((m) => {
            const link = document.createElement("a");
            link.className = "right-toc-member";
            link.href = `#${m.el.id}`;
            link.dataset.kind = m.el.dataset.kind || "property";
            if (isDeprecated(m.el)) link.dataset.deprecated = "true";
            const icon = document.createElement("span");
            icon.className = "right-toc-member-icon";
            const label = document.createElement("span");
            label.className = "right-toc-member-name";
            label.textContent = m.display;
            link.appendChild(icon);
            link.appendChild(label);
            nav.appendChild(link);
        });
    });

    const targets = [];
    sections.forEach((sec) => {
        targets.push({ el: sec.heading, link: nav.querySelector(`a[href="#${cssEsc(sec.heading.id)}"]`) });
        sec.members.forEach((m) => {
            targets.push({
                el: m.el,
                link: nav.querySelector(`a[href="#${cssEsc(m.el.id)}"]`),
            });
        });
    });

    function cssEsc(id) {
        if (window.CSS && CSS.escape) return CSS.escape(id);
        return id.replace(/([\\\.#:\[\]\(\)\!\?\@\&\=\,\/\*])/g, "\\$1");
    }

    if ("IntersectionObserver" in window) {
        const visible = new Set();
        const observer = new IntersectionObserver(
            (entries) => {
                entries.forEach((entry) => {
                    if (entry.isIntersecting) {
                        visible.add(entry.target);
                    } else {
                        visible.delete(entry.target);
                    }
                });
                let active = null;
                let activeTop = Infinity;
                visible.forEach((el) => {
                    const top = el.getBoundingClientRect().top;
                    if (top < activeTop) {
                        activeTop = top;
                        active = el;
                    }
                });
                if (!active) return;
                nav.querySelectorAll(".active").forEach((n) => n.classList.remove("active"));
                const t = targets.find((t) => t.el === active);
                if (t && t.link) t.link.classList.add("active");
            },
            { rootMargin: "-20% 0px -70% 0px", threshold: 0 },
        );
        targets.forEach((t) => observer.observe(t.el));
    }

    if (window.matchMedia && window.matchMedia("(max-width: 1279px)").matches) {
        const toggle = document.createElement("button");
        toggle.type = "button";
        toggle.className = "right-toc-toggle";
        toggle.setAttribute("aria-label", "Toggle on-this-page");
        toggle.innerHTML =
            '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>';
        toggle.addEventListener("click", () => {
            rail.classList.toggle("is-open");
        });
        document.body.appendChild(toggle);
        rail.addEventListener("click", (e) => {
            if (e.target.tagName === "A") rail.classList.remove("is-open");
        });
    }
})();
