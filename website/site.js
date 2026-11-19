// TeXoria's website: theme switch, download link for the visitor's system, install tabs, copy
// buttons and the documentation's menu. Without JavaScript the page still works: the theme
// follows the system, the download button opens the latest release and every install panel is
// shown.
"use strict";

const REPO = "Herbrant/TeXoria";
const root = document.documentElement;
const dark = window.matchMedia("(prefers-color-scheme: dark)");

// Theme: "paper" or "ink" when picked by the visitor, otherwise the system's.
function currentTheme() {
  return root.dataset.theme || (dark.matches ? "ink" : "paper");
}

function applyTheme(theme) {
  root.dataset.theme = theme;
  // The screenshot follows the page's theme, not the system's.
  const source = document.getElementById("screenshot-dark");
  if (source) source.media = theme === "ink" ? "all" : "not all";
  try {
    localStorage.setItem("texoria-theme", theme);
  } catch (_) {}
}

// The visitor's system: "macos", "windows", "linux" or null (phones, tablets, unknown).
function detectSystem() {
  const ua = navigator.userAgent;
  if (/iPhone|iPad|iPod|Android/i.test(ua)) return null;
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || ua;
  if (/Mac/i.test(platform)) return "macos";
  if (/Win/i.test(platform)) return "windows";
  if (/Linux|X11|CrOS/i.test(platform)) return "linux";
  return null;
}

const SYSTEM_NAMES = { macos: "macOS", windows: "Windows", linux: "Linux" };

// The release asset for a system: names are texoria-<version>-<os>-<arch>.<ext> (release.yml).
// Browsers rarely tell the architecture, so macOS gets Apple silicon and Linux x86_64 unless
// the user agent says otherwise; the hint links to the other builds.
function assetFor(system, version) {
  const ua = navigator.userAgent;
  switch (system) {
    case "macos":
      return { name: `texoria-${version}-macos-aarch64.dmg`, label: "macOS (Apple silicon)" };
    case "windows":
      return { name: `texoria-${version}-windows-x86_64-setup.exe`, label: "Windows" };
    case "linux": {
      const arch = /aarch64|arm64/i.test(ua) ? "aarch64" : "x86_64";
      return { name: `texoria-${version}-linux-${arch}.AppImage`, label: `Linux (${arch} AppImage)` };
    }
    default:
      return null;
  }
}

async function setUpDownload(system) {
  const button = document.getElementById("download");
  const label = document.getElementById("download-label");
  const hint = document.getElementById("download-hint");
  if (!button || !system) return;
  label.textContent = `Download for ${SYSTEM_NAMES[system]}`;
  try {
    const response = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) return;
    const release = await response.json();
    const version = String(release.tag_name || "").replace(/^v/, "");
    const asset = assetFor(system, version);
    const found = asset && (release.assets || []).find((a) => a.name === asset.name);
    if (!found) return;
    button.href = found.browser_download_url;
    hint.replaceChildren(
      `Version ${version} for ${asset.label} · `,
      link(release.html_url, "release notes"),
      " · ",
      link("#install", "other platforms"),
    );
  } catch (_) {
    // Offline or rate-limited: the button keeps pointing to the latest release's page.
  }
}

function link(href, text) {
  const a = document.createElement("a");
  a.href = href;
  a.textContent = text;
  return a;
}

// Install tabs (WAI-ARIA tabs: arrow keys move between them).
function setUpTabs(system) {
  const tabs = Array.from(document.querySelectorAll('.tabs [role="tab"]'));
  if (tabs.length === 0) return;
  function select(tab, focus) {
    for (const t of tabs) {
      const selected = t === tab;
      t.setAttribute("aria-selected", String(selected));
      t.tabIndex = selected ? 0 : -1;
      document.getElementById(t.getAttribute("aria-controls")).hidden = !selected;
    }
    if (focus) tab.focus();
  }
  tabs.forEach((tab, i) => {
    tab.addEventListener("click", () => select(tab, false));
    tab.addEventListener("keydown", (event) => {
      const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
      if (!step) return;
      event.preventDefault();
      select(tabs[(i + step + tabs.length) % tabs.length], true);
    });
  });
  select(document.getElementById(`tab-${system}`) || tabs[0], false);
}

function setUpCopyButtons() {
  for (const button of document.querySelectorAll(".command .copy")) {
    button.addEventListener("click", async () => {
      const code = button.parentElement.querySelector("code");
      try {
        await navigator.clipboard.writeText(code.textContent);
        button.textContent = "Copied";
      } catch (_) {
        // No clipboard access (insecure context, denied): select the command instead.
        const range = document.createRange();
        range.selectNodeContents(code);
        window.getSelection().removeAllRanges();
        window.getSelection().addRange(range);
        button.textContent = "Selected";
      }
      setTimeout(() => (button.textContent = "Copy"), 1600);
    });
  }
}

// The documentation's sidebar is a menu on narrow screens, closed until opened (without
// JavaScript it stays open).
function setUpDocsMenu() {
  const menu = document.querySelector(".docs-menu");
  if (!menu) return;
  const narrow = window.matchMedia("(max-width: 860px)");
  const update = () => (menu.open = !narrow.matches);
  update();
  narrow.addEventListener("change", update);
}

const system = detectSystem();
if (root.dataset.theme) applyTheme(root.dataset.theme);
document.getElementById("theme-toggle").addEventListener("click", () => {
  applyTheme(currentTheme() === "ink" ? "paper" : "ink");
});
setUpTabs(system);
setUpCopyButtons();
setUpDocsMenu();
setUpDownload(system);
