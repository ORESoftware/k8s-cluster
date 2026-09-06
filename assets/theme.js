(() => {
  const allowed = new Set(["dark", "medium", "light"]);
  const stored = localStorage.getItem("akrion-theme");
  const theme = allowed.has(stored) ? stored : "dark";
  document.documentElement.dataset.theme = theme;
})();
