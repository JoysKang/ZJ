// Design-time tool: Node.js + sharp, and macOS iconutil. No runtime app dependency.
const fs = require('node:fs/promises');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const sharp = require('sharp');

async function main() {
  const root = path.resolve(__dirname, '..');
  const source = path.join(root, 'crates/app/assets/app-icon/bamboo.svg');
  const output = path.join(root, 'target/app-icon');
  const iconset = path.join(output, 'bamboo.iconset');
  await fs.mkdir(iconset, { recursive: true });
  for (const size of [16, 32, 128, 256, 512]) {
    for (const scale of [1, 2]) {
      const pixels = size * scale;
      const suffix = scale === 2 ? '@2x' : '';
      await sharp(source, { density: 144 })
        .resize(pixels, pixels)
        .png()
        .toFile(path.join(iconset, `icon_${size}x${size}${suffix}.png`));
    }
  }
  const preview = path.join(output, 'bamboo.png');
  await fs.copyFile(path.join(iconset, 'icon_512x512@2x.png'), preview);
  execFileSync('/usr/bin/iconutil', [
    '-c', 'icns', iconset, '-o',
    path.join(root, 'crates/app/assets/app-icon/bamboo.icns'),
  ]);
  process.stdout.write(`Preview: ${preview}\n`);
}

main().catch(error => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
