// Design-time tool: Node.js + sharp; macOS iconutil when present, otherwise tools/make_icns.py.
// No runtime app dependency. Regenerates everything under crates/app/assets/app-icon/ from the
// SVG sources: the iconset (16, 16@2x and 32 from the simplified small art, the rest from the
// full art), zj.icns, and the cursor-off frame used by the optional Dock blink.
const fs = require('node:fs/promises');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const sharp = require('sharp');

async function render(source, pixels, output) {
  await sharp(source, { density: 288 }).resize(pixels, pixels).png().toFile(output);
}

async function main() {
  const root = path.resolve(__dirname, '..');
  const art = path.join(root, 'crates/app/assets/app-icon');
  const full = path.join(art, 'zj.svg');
  const small = path.join(art, 'zj-small.svg');
  const iconset = path.join(art, 'ZJ.iconset');
  await fs.mkdir(iconset, { recursive: true });
  for (const size of [16, 32, 128, 256, 512]) {
    for (const scale of [1, 2]) {
      const pixels = size * scale;
      const suffix = scale === 2 ? '@2x' : '';
      // At 32 px and below the full art turns to noise; the small art is drawn for the grid.
      await render(pixels <= 32 ? small : full, pixels, path.join(iconset, `icon_${size}x${size}${suffix}.png`));
    }
  }
  await render(path.join(art, 'zj-cursor-off.svg'), 512, path.join(art, 'dock-cursor-off.png'));
  const icns = path.join(art, 'zj.icns');
  try {
    execFileSync('/usr/bin/iconutil', ['-c', 'icns', iconset, '-o', icns]);
  } catch {
    execFileSync('python3', [path.join(root, 'tools/make_icns.py'), iconset, icns], { stdio: 'inherit' });
  }
  process.stdout.write(`Icon: ${icns}\n`);
}

main().catch(error => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
