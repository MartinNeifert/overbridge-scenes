(function () {
  const KEY = "ob-scenes:theme";

  function storedTheme() {
    try {
      return localStorage.getItem(KEY) === "light" ? "light" : "dark";
    } catch {
      return "dark";
    }
  }

  function applyTheme(theme) {
    const light = theme === "light";
    if (light) document.documentElement.dataset.theme = "light";
    else delete document.documentElement.dataset.theme;
    try {
      localStorage.setItem(KEY, light ? "light" : "dark");
    } catch (_) {}
    for (const btn of document.querySelectorAll(".ob-theme-toggle")) {
      btn.textContent = light ? "Dark" : "Light";
      btn.setAttribute("aria-label", light ? "Switch to dark theme" : "Switch to light theme");
      btn.setAttribute("title", light ? "Dark theme" : "Light theme");
    }
  }

  function toggleTheme() {
    applyTheme(storedTheme() === "light" ? "dark" : "light");
  }

  applyTheme(storedTheme());

  document.addEventListener("DOMContentLoaded", () => {
    applyTheme(storedTheme());
    for (const btn of document.querySelectorAll(".ob-theme-toggle")) {
      btn.addEventListener("click", toggleTheme);
    }
  });
})();
