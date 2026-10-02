// node render-share.cjs [docs-directory]
// Real production UI captures must already exist in target/showcase.
const {chromium} = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const docs = path.resolve(process.argv[2] || path.join(root, 'docs'));
const media = path.join(docs, 'media');
const png = name => `data:image/png;base64,${fs.readFileSync(path.join(root, 'target/showcase', name)).toString('base64')}`;

(async () => {
  fs.mkdirSync(media, {recursive:true});
  const browser = await chromium.launch({channel:'chrome', headless:true});
  try {
    const page = await browser.newPage({viewport:{width:1200,height:630},deviceScaleFactor:1});
    for (const name of ['gallery-dark','gallery-light','theme','editor']) {
      fs.copyFileSync(path.join(root, 'target/showcase', `${name}.png`), path.join(media, `${name}.png`));
      await page.setContent(`<img src="${png(`${name}.png`)}">`);
      const encoded = await page.locator('img').evaluate(async image => {
        await image.decode();
        const canvas = document.createElement('canvas');
        canvas.width = image.naturalWidth;
        canvas.height = image.naturalHeight;
        canvas.getContext('2d').drawImage(image,0,0);
        return canvas.toDataURL('image/webp',0.92).split(',')[1];
      });
      fs.writeFileSync(path.join(media, `${name}.webp`), Buffer.from(encoded,'base64'));
    }
    const font = `data:font/ttf;base64,${fs.readFileSync(path.join(root,'assets/fonts/Rajdhani-SemiBold.ttf')).toString('base64')}`;
    const icon = `data:image/png;base64,${fs.readFileSync(path.join(docs,'assets/icon.png')).toString('base64')}`;
    await page.setContent(`<!doctype html><html lang="en"><head><style>
      @font-face{font-family:Rajdhani;src:url('${font}');font-weight:600}*{box-sizing:border-box}
      body{margin:0;width:1200px;height:630px;overflow:hidden;background:#081923;color:#e6f4fa;font-family:Rajdhani,'Segoe UI',sans-serif}
      header{position:absolute;top:27px;left:44px;right:44px;display:flex;align-items:center;justify-content:space-between}
      .brand{display:flex;align-items:center;gap:16px;font-size:57px;font-weight:600;letter-spacing:-1px}.brand img{width:58px;height:58px}
      .intro{font-family:Rajdhani,sans-serif;font-size:23px;color:#b9d2dc;line-height:1.6;text-align:right}
      .shot{position:absolute;left:44px;top:136px;width:1112px;border:1px solid #2d6c80;border-radius:8px;display:block}
      .site{position:absolute;right:44px;bottom:20px;font-family:Rajdhani,sans-serif;font-size:21px;background:#081923e8;color:#80dcff;padding:9px 14px;border-radius:5px}
    </style></head><body><header><div class="brand"><img src="${icon}" alt="">TrontSnap</div><div class="intro">Screenshots. Recording. Offline OCR.<br>Your whole history, in one gallery.</div></header><img class="shot" src="${png('gallery-dark.png')}" alt="Current TrontSnap gallery"><div class="site">tront.xyz/trontsnap</div></body></html>`);
    await page.evaluate(async () => {await document.fonts.ready;await Promise.all([...document.images].map(image=>image.decode()));});
    await page.screenshot({path:path.join(media,'og-20261002.png')});
    await page.setContent(`<style>body{margin:0;width:1200px;height:630px;background:#062432;overflow:hidden}img{display:block;width:1200px;height:630px;object-fit:cover;object-position:top}</style><img src="${png('gallery-dark.png')}" alt="Current TrontSnap gallery">`);
    await page.locator('img').evaluate(image=>image.decode());
    await page.screenshot({path:path.join(media,'card-20261002.png')});
    console.log('PASS: four production captures, four WebP assets, OG and games card rendered.');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
