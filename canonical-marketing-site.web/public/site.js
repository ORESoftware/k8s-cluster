const APP_SCHEME = 'https';
const APP_HOST = 'app.canonical.plus';
const APP_ORIGIN = [APP_SCHEME, APP_HOST].join('://');
const QUOTE_PATH = '/u/quote';
const quoteUrl = new URL(QUOTE_PATH, APP_ORIGIN);
const signInUrl = new URL(QUOTE_PATH, APP_ORIGIN);

const configureApplicationLinks = () => {
  const navContact = document.getElementById('nav-contact');

  if (navContact instanceof HTMLAnchorElement) {
    navContact.href = quoteUrl.href;
    navContact.textContent = 'Get a quote · under 5 min';
    navContact.setAttribute('aria-label', 'Get a quote in less than 5 minutes');
    navContact.dataset.applicationLink = 'quote';

    let signIn = document.getElementById('nav-sign-in');
    if (!(signIn instanceof HTMLAnchorElement)) {
      signIn = document.createElement('a');
      signIn.id = 'nav-sign-in';
      signIn.className = 'nav__link';
      navContact.before(signIn);
    }
    signIn.href = signInUrl.href;
    signIn.textContent = 'Sign in';
    signIn.dataset.applicationLink = 'sign-in';
  }

  const heroPrimary = document.getElementById('hero-cta-primary');
  if (heroPrimary instanceof HTMLAnchorElement) {
    heroPrimary.href = quoteUrl.href;
    heroPrimary.setAttribute('aria-label', 'Get a quote in less than 5 minutes');
    heroPrimary.dataset.applicationLink = 'quote';

    const labelNode = Array.from(heroPrimary.childNodes).find(
      (node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim(),
    );
    if (labelNode) {
      labelNode.textContent = '\n            Get a quote in under 5 min\n            ';
    }
  }
};

configureApplicationLinks();

const nav = document.getElementById('main-nav');

if (nav) {
  window.addEventListener(
    'scroll',
    () => {
      nav.style.borderBottomColor = window.scrollY > 10
        ? 'rgba(255,255,255,0.1)'
        : 'rgba(255,255,255,0.06)';
    },
    { passive: true },
  );
}

const skipLink = document.querySelector('.skip-link');
const mainContent = document.getElementById('main-content');

if (skipLink instanceof HTMLAnchorElement && mainContent instanceof HTMLElement) {
  skipLink.addEventListener('click', () => {
    // The native fragment remains the navigation authority. Explicit focus
    // makes the destination deterministic across Chromium and assistive-tech
    // combinations while the static link still works without JavaScript.
    mainContent.focus({ preventScroll: true });
  });
}

const toggle = document.getElementById('nav-toggle');
const links = document.getElementById('nav-links');
const mobileNavigation = window.matchMedia('(max-width: 768px)');

if (toggle && links) {
  toggle.type = 'button';
  toggle.setAttribute('aria-controls', links.id);

  const setNavigationOpen = (open, { restoreFocus = false } = {}) => {
    const nextOpen = Boolean(open && mobileNavigation.matches);
    links.classList.toggle('nav__links--open', nextOpen);
    toggle.setAttribute('aria-expanded', String(nextOpen));
    toggle.setAttribute('aria-label', nextOpen ? 'Close navigation' : 'Open navigation');

    if (restoreFocus) {
      toggle.focus();
    }
  };

  setNavigationOpen(false);

  toggle.addEventListener('click', () => {
    setNavigationOpen(toggle.getAttribute('aria-expanded') !== 'true');
  });

  links.addEventListener('click', (event) => {
    if (event.target instanceof HTMLAnchorElement) {
      setNavigationOpen(false);
    }
  });

  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && toggle.getAttribute('aria-expanded') === 'true') {
      setNavigationOpen(false, { restoreFocus: true });
    }
  });

  mobileNavigation.addEventListener('change', () => setNavigationOpen(false));
}
