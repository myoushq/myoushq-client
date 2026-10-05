// Copy the SPAKE2 WebAssembly build (clients/rust/wasm/pkg, built by
// clients/rust/wasm/build.sh) into ./wasm, where src/pake.ts loads it.
import { cpSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const from = fileURLToPath(new URL("../../rust/wasm/pkg/", import.meta.url));
const to = fileURLToPath(new URL("../wasm/", import.meta.url));
if (!existsSync(from)) {
  console.error(`missing ${from}: run clients/rust/wasm/build.sh first`);
  process.exit(1);
}
mkdirSync(to, { recursive: true });
cpSync(from, to, { recursive: true });
// wasm-bindgen's nodejs build is CommonJS, but this package is "type":
// "module"; mark the directory so Node loads it as CommonJS.
writeFileSync(new URL("../wasm/package.json", import.meta.url), '{ "type": "commonjs" }\n');
console.log(`copied ${from} -> ${to}`);
