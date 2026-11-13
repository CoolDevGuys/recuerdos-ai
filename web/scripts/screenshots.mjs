// Takes the screenshots in docs/web-ui.md from a running daemon, and the
// accessibility checks' evidence: the map under simulated colour-vision
// deficiencies, dark mode, and phone width.
//
//   RECUERDOS_URL=http://127.0.0.1:7070 RECUERDOS_KEY=ra_live_… OUT=../docs/images node scripts/screenshots.mjs
//
// Point it at a store filled by scripts/seed-demo.mjs, never a real one:
// the pictures show memory text.
import { chromium } from "@playwright/test";

const base = process.env.RECUERDOS_URL ?? "http://127.0.0.1:7070";
const key = process.env.RECUERDOS_KEY;
const out = process.env.OUT ?? "screenshots";
const extra = process.env.EXTRA === "1";
if (!key) throw new Error("Set RECUERDOS_KEY to the demo user's key.");

// Machado et al. 2009 at full severity, as SVG colour matrices (sRGB; close
// enough for a visual check, the exact check is in palette.test.ts).
const CVD = {
  protanopia: "0.152 1.053 -0.205 0 0  0.115 0.786 0.099 0 0  -0.004 -0.048 1.052 0 0  0 0 0 1 0",
  deuteranopia: "0.367 0.861 -0.228 0 0  0.280 0.673 0.047 0 0  -0.012 0.043 0.969 0 0  0 0 0 1 0",
  tritanopia: "1.256 -0.077 -0.179 0 0  -0.078 0.931 0.148 0 0  0.005 0.691 0.304 0 0  0 0 0 1 0",
};

async function signedIn(browser, options) {
  const context = await browser.newContext({ deviceScaleFactor: 2, ...options });
  const page = await context.newPage();
  await page.goto(`${base}/ui/`);
  await page.getByLabel("API key").fill(key);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.getByRole("link", { name: "List" }).waitFor();
  return page;
}

async function openMap(page) {
  await page.getByRole("link", { name: "Map" }).click();
  await page.getByRole("application", { name: /Map of/ }).waitFor();
  await page.waitForTimeout(800);
}

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
try {
  const desktop = { viewport: { width: 1280, height: 800 } };

  let page = await signedIn(browser, desktop);
  await page.screenshot({ path: `${out}/list.png` });
  await openMap(page);
  await page.screenshot({ path: `${out}/map-topics.png` });
  const map = page.getByRole("application", { name: /Map of/ });
  await map.focus();
  await page.keyboard.press("2");
  await page.waitForTimeout(900);
  await page.screenshot({ path: `${out}/map-clusters.png` });
  await page.keyboard.press("3");
  await page.waitForTimeout(900);
  await page.screenshot({ path: `${out}/map-memories.png` });
  await page.getByRole("link", { name: /Needs review/ }).click();
  await page.getByRole("heading", { name: /Needs review/ }).waitFor();
  await page.screenshot({ path: `${out}/review.png` });
  await page.getByRole("link", { name: "Projects" }).click();
  await page.getByRole("heading", { name: "quill" }).waitFor();
  await page.screenshot({ path: `${out}/projects.png` });
  await page.getByRole("link", { name: "Topics" }).click();
  await page.getByRole("button", { name: "Add topic" }).waitFor();
  await page.screenshot({ path: `${out}/topics.png` });
  await page.getByRole("link", { name: "Categories" }).click();
  await page.getByRole("table").waitFor();
  await page.screenshot({ path: `${out}/categories.png` });
  await page.context().close();

  page = await signedIn(browser, { ...desktop, colorScheme: "dark" });
  await openMap(page);
  await page.screenshot({ path: `${out}/map-dark.png` });
  await page.context().close();

  page = await signedIn(browser, { viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true });
  await page.screenshot({ path: `${out}/phone-list.png`, fullPage: false });
  await page.getByRole("button", { name: /^Edit / }).first().click().catch(async () => {
    await page.locator("li button").first().click();
  });
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${out}/phone-editor.png` });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  console.log(`phone: horizontal overflow ${overflow}px`);
  await page.context().close();

  if (extra) {
    page = await signedIn(browser, desktop);
    await openMap(page);
    for (const [name, matrix] of Object.entries(CVD)) {
      await page.evaluate(
        ([id, values]) => {
          document.getElementById("cvd")?.remove();
          const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
          svg.id = "cvd";
          svg.setAttribute("style", "position:absolute;width:0;height:0");
          svg.innerHTML = `<filter id="${id}"><feColorMatrix type="matrix" values="${values}"/></filter>`;
          document.body.append(svg);
          document.documentElement.style.filter = `url(#${id})`;
        },
        [name, matrix],
      );
      await page.waitForTimeout(300);
      await page.screenshot({ path: `${out}/cvd-${name}.png` });
    }
    await page.context().close();
  }
} finally {
  await browser.close();
}
console.log(`Screenshots in ${out}`);
