import sharp from "sharp";
import { readFileSync } from "fs";

const svg = readFileSync("tools/app-icon.svg");
await sharp(svg, { density: 300 })
  .resize(1024, 1024)
  .png()
  .toFile("src-tauri/app-icon.png");
console.log("app-icon.png written");
