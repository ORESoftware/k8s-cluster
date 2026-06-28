let config = {};
const allowedThemes = new Set(["dark", "medium", "light"]);
const authStatus = document.getElementById("auth-status");
const authUser = document.getElementById("auth-user");
const authForm = document.getElementById("auth-form");
let supabaseClient = null;
let currentAccessToken = null;

function activeTheme() {
  const current = document.documentElement.dataset.theme;
  return allowedThemes.has(current) ? current : "dark";
}

function setTheme(theme) {
  const next = allowedThemes.has(theme) ? theme : "dark";
  document.documentElement.dataset.theme = next;
  localStorage.setItem("akrion-theme", next);
  document.querySelectorAll("[data-theme-option]").forEach((button) => {
    const selected = button.dataset.themeOption === next;
    button.classList.toggle("is-active", selected);
    button.setAttribute("aria-checked", String(selected));
  });
}

function setAuthStatus(message, kind = "") {
  if (!authStatus) return;
  authStatus.textContent = message;
  authStatus.className = `auth-status ${kind}`;
}

function setUser(session) {
  currentAccessToken = session?.access_token || null;
  if (!authUser) return;
  const email = session?.user?.email || "Signed out";
  authUser.querySelector("span").textContent = email;
}

async function refreshSession() {
  if (!supabaseClient) return;
  const { data } = await supabaseClient.auth.getSession();
  setUser(data.session);
  if (data.session) setAuthStatus("Signed in", "ready");
}

async function loadConfig() {
  const configUrl = document.documentElement.dataset.configUrl || "/config";
  try {
    const response = await fetch(configUrl, {
      cache: "no-store",
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new Error(`config ${response.status}`);
    return response.json();
  } catch (_error) {
    setAuthStatus("Config unavailable", "offline");
    return {};
  }
}

async function initializeAuth() {
  config = await loadConfig();
  if (window.supabase && config.supabase?.enabled) {
    supabaseClient = window.supabase.createClient(config.supabase.url, config.supabase.anon_key);
    supabaseClient.auth.onAuthStateChange((_event, session) => {
      setUser(session);
      setAuthStatus(session ? "Signed in" : "Signed out", session ? "ready" : "");
    });
    refreshSession();
  } else {
    setAuthStatus("Set SUPABASE_URL and SUPABASE_ANON_KEY", "offline");
  }
}

document.body.addEventListener("htmx:configRequest", (event) => {
  if (currentAccessToken) {
    event.detail.headers.Authorization = `Bearer ${currentAccessToken}`;
  }
});

authForm?.addEventListener("submit", async (event) => {
  event.preventDefault();
  await runAuthAction("password");
});

document.querySelectorAll("[data-auth-action]").forEach((button) => {
  button.addEventListener("click", async () => runAuthAction(button.dataset.authAction));
});

document.querySelectorAll("[data-theme-option]").forEach((button) => {
  button.addEventListener("click", () => setTheme(button.dataset.themeOption));
});

async function runAuthAction(action) {
  if (!supabaseClient) {
    setAuthStatus("Supabase unavailable", "offline");
    return;
  }

  const email = document.getElementById("auth-email")?.value?.trim();
  const password = document.getElementById("auth-password")?.value || "";
  setAuthStatus("Working...", "");

  try {
    if (action === "password") {
      const { error } = await supabaseClient.auth.signInWithPassword({ email, password });
      if (error) throw error;
      setAuthStatus("Signed in", "ready");
    } else if (action === "magic") {
      const { error } = await supabaseClient.auth.signInWithOtp({ email });
      if (error) throw error;
      setAuthStatus("Magic link sent", "ready");
    } else if (action === "signup") {
      const { error } = await supabaseClient.auth.signUp({ email, password });
      if (error) throw error;
      setAuthStatus("Account requested", "ready");
    } else if (action === "signout") {
      const { error } = await supabaseClient.auth.signOut();
      if (error) throw error;
      setUser(null);
      setAuthStatus("Signed out", "");
    }
  } catch (error) {
    setAuthStatus(error.message || "Auth failed", "offline");
  }
}

document.body.addEventListener("htmx:afterSwap", () => {
  window.lucide?.createIcons();
  setTheme(activeTheme());
});
setTheme(activeTheme());
window.lucide?.createIcons();
initializeAuth();
