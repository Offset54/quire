// Decodes fallback-transport tiles off the UI thread (ADR-0004: decoding on
// the UI thread caused janky frames) and hands back ready ImageBitmaps.

import { decodePixels, parseHeader, type TileId } from "./protocol";

export interface DecodedTile {
  id: TileId;
  bitmap: ImageBitmap;
}

self.onmessage = async (e: MessageEvent<ArrayBuffer>) => {
  try {
    const header = parseHeader(e.data);
    const pixels = decodePixels(e.data, header);
    const bitmap = await createImageBitmap(new ImageData(pixels, header.width, header.height));
    const out: DecodedTile = { id: header, bitmap };
    (self as unknown as Worker).postMessage(out, [bitmap]);
  } catch (err) {
    console.error("tile decode failed", err);
  }
};
