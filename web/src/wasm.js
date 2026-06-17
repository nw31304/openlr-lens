export async function initWasm() {
  const mod = await import('./wasm/openlr_wasm.js');
  await mod.default();
  return new mod.Decoder();
}
