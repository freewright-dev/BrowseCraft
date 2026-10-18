const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const forwarded = [];
class Element { setAttribute() {} }
const context = {
  URL, Request, TypeError, Promise, Element,
  location: { href: 'https://site.example/', hostname: 'site.example' },
  document: { baseURI: 'https://site.example/', createElement: () => ({}) },
  navigator: {}, XMLHttpRequest: class { open() {} },
  fetch: async request => { forwarded.push(request); return {}; },
};
for (const tag of ['Image','Script','IFrame','Embed','Object','Link','Audio','Video','Source']) {
  context[`HTML${tag}Element`] = class extends Element {};
}
context.window = context;
vm.createContext(context);
vm.runInContext(fs.readFileSync(0, 'utf8'), context);
(async () => {
  const blocked = 'https://blocked.example/track';
  const ForeignRequest = vm.runInNewContext('(class extends Request {})', { Request });
  for (const input of [blocked, new URL(blocked), new Request(blocked), new ForeignRequest(blocked)]) {
    await assert.rejects(context.fetch(input), /Blocked by BrowseCraft/);
  }
  assert.equal(forwarded.length, 0);
  let conversions = 0;
  await context.fetch({ toString() { conversions++; return 'https://site.example/ok'; } }, { method: 'POST', body: 'a=b' });
  assert.equal(conversions, 1);
  assert.equal(forwarded.length, 1);
  assert.equal(forwarded[0].method, 'POST');
  assert.equal(await forwarded[0].text(), 'a=b');
  await assert.rejects(context.fetch({ toString() { throw new Error('conversion failed'); } }), /conversion failed/);
})().catch(error => { console.error(error); process.exitCode = 1; });
