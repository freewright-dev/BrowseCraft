const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
// Records which src attributes the page script lets through.
const loaded = [];
class Element { setAttribute(name, value) { loaded.push(value); } }
const context = {
  URL, Request, TypeError, Promise, Element,
  location: { href: 'https://site.example/', hostname: 'site.example' },
  document: { baseURI: 'https://site.example/', createElement: () => ({}) },
  navigator: {}, XMLHttpRequest: class { open() {} },
  fetch: async () => ({}),
};
for (const tag of ['Image','Script','IFrame','Embed','Object','Link','Audio','Video','Source']) {
  context[`HTML${tag}Element`] = class extends Element {};
}
context.window = context;
vm.createContext(context);
vm.runInContext(fs.readFileSync(0, 'utf8'), context);
const load = url => {
  const img = new context.HTMLImageElement();
  img.tagName = 'IMG';
  img.setAttribute('src', url);
  return loaded.includes(url);
};
// The filter is `||cdn.example^ads`: blocked when a separator and "ads"
// directly follow the host. The host text in the user info must not be
// taken for the host itself.
assert.equal(load('https://cdn.example/ads'), false);
assert.equal(load('https://img.cdn.example/ads'), false);
assert.equal(load('https://cdn.example/x/ads'), true);
assert.equal(load('https://cdn.example:ads@cdn.example/ok'), true);
assert.equal(load('https://cdn.example:8080/ok'), true);
assert.equal(load('https://other.example/ads'), true);
