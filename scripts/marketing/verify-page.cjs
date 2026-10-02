// node verify-page.cjs <portfolio-checkout>
// The project's docs are mounted at /trontsnap, as on GitHub Pages.
const {chromium} = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '../..');
const site = path.resolve(process.argv[2]);
const out = path.join(root,'target/page-review');
const docs = path.join(root,'docs');
const checks = [];

(async()=>{
  fs.mkdirSync(out,{recursive:true});
  const server = http.createServer((req,res)=>{
    const url = new URL(req.url,'http://localhost');
    const product = url.pathname.startsWith('/trontsnap/');
    const base = product ? docs : site;
    const relative = product ? url.pathname.slice('/trontsnap'.length) : url.pathname;
    let file = path.resolve(base,'.'+decodeURIComponent(relative));
    if (!file.startsWith(base+path.sep) && file !== base) {res.writeHead(403);res.end();return;}
    try {
      if (fs.statSync(file).isDirectory()) file = path.join(file,'index.html');
      res.setHeader('Content-Type', ({'.html':'text/html; charset=utf-8','.js':'text/javascript','.css':'text/css','.png':'image/png','.webp':'image/webp','.ttf':'font/ttf','.jpg':'image/jpeg'})[path.extname(file)] || 'application/octet-stream');
      res.end(fs.readFileSync(file));
    } catch {res.writeHead(404);res.end('Missing');}
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({channel:'chrome',headless:true});
  try {
    for (const width of [1366,390,320]) {
      const page = await browser.newPage({viewport:{width,height:width===1366?768:844},deviceScaleFactor:1});
      const errors=[];
      page.on('pageerror',error=>errors.push(error.message));
      await page.route('**/*',route=>route.request().url().startsWith(base) ? route.continue() : route.abort());
      await page.goto(base+'/trontsnap/',{waitUntil:'networkidle'});
      await page.evaluate(()=>document.fonts.ready);
      assert.deepEqual(errors,[],`product script errors at ${width}`);
      assert.equal(await page.locator('h1').count(),1);
      assert.equal(await page.locator('main').count(),1);
      assert.equal(await page.locator('.shotmock').count(),0);
      assert.equal(await page.locator('meta[property="og:image"]').getAttribute('content'),'https://tront.xyz/trontsnap/media/og-20261002.png');
      assert.equal(await page.evaluate(()=>document.body.innerText.includes('\u2014')),false);
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`horizontal overflow at ${width}`);
      assert.ok(await page.locator('h1').evaluate(h=>h.getBoundingClientRect().height/parseFloat(getComputedStyle(h).lineHeight)<2.2),`hero headline wraps past two lines at ${width}`);
      const download = await page.locator('.hero .t-btn--primary').boundingBox();
      assert.ok(download.y+download.height<=844,`hero download below fold at ${width}`);
      const images = page.locator('.product-shot img');
      assert.equal(await images.count(),4);
      for (const img of await images.all()) {
        await img.scrollIntoViewIfNeeded();
        await img.evaluate(image=>image.decode());
        assert.ok(await img.evaluate(image=>image.naturalWidth>=1200));
      }
      await page.evaluate(()=>scrollTo(0,0));
      await page.screenshot({path:path.join(out,`product-${width}-fold.png`)});
      await page.screenshot({path:path.join(out,`product-${width}.png`),fullPage:true});
      await page.getByRole('button',{name:'Roll theme'}).click();
      assert.deepEqual(errors,[]);
      await page.getByRole('link',{name:'Features',exact:true}).click();
      await page.getByRole('link',{name:'Open the full-size gallery screenshot'}).evaluate(link=>{
        if (!link.href.endsWith('/media/gallery-dark.png')) throw new Error('Screenshot link drift');
      });
      await page.goto(base+'/games/',{waitUntil:'networkidle'});
      const card = page.locator('a.item[href="/trontsnap/"]');
      assert.equal(await card.count(),1);
      await card.scrollIntoViewIfNeeded();
      assert.equal(await card.locator('img').getAttribute('src'),'/img/trontsnap-card-20261002.png');
      await card.locator('img').evaluate(image=>image.decode());
      assert.ok((await card.innerText()).includes('offline OCR'));
      await card.screenshot({path:path.join(out,`card-${width}.png`)});
      checks.push({width,product:'PASS',card:'PASS',images:4,scriptErrors:errors});
      await page.close();
    }
    fs.writeFileSync(path.join(out,'checks.json'),JSON.stringify(checks,null,2)+'\n');
    console.log('PASS: product page and games card at 1366, 390 and 320 px; real images, OG metadata, links, theme action, no overflow.');
  } finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
})().catch(error=>{console.error(error);process.exitCode=1;});
