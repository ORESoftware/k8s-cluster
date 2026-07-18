const nav = document.getElementById('main-nav');

if (nav) {
  window.addEventListener('scroll', () => {
    nav.style.borderBottomColor = window.scrollY > 10
      ? 'rgba(255,255,255,0.1)'
      : 'rgba(255,255,255,0.06)';
  });
}

const toggle = document.getElementById('nav-toggle');
const links = document.getElementById('nav-links');

if (toggle && links) {
  toggle.addEventListener('click', () => {
    links.classList.toggle('nav__links--open');
  });
}
