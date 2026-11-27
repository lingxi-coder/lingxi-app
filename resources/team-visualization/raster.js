// Read dimensions without invoking a browser image decoder. Card images are static.
function png(bytes, data) {
  if (bytes.length < 33 || data.getUint32(0) !== 0x89504e47 || data.getUint32(4) !== 0x0d0a1a0a
      || data.getUint32(8) !== 13 || data.getUint32(12) !== 0x49484452) return;
  for (let offset = 8; offset + 12 <= bytes.length;) {
    const length = data.getUint32(offset), type = data.getUint32(offset + 4);
    if (length > bytes.length - offset - 12 || type === 0x6163544c) return; // acTL
    if (type === 0x49454e44) return [data.getUint32(16), data.getUint32(20)]; // IEND
    offset += 12 + length;
  }
}

function jpeg(bytes, data) {
  if (bytes[0] !== 0xff || bytes[1] !== 0xd8) return;
  for (let offset = 2; offset + 4 < bytes.length;) {
    if (bytes[offset++] !== 0xff) return;
    while (bytes[offset] === 0xff) offset++;
    const type = bytes[offset++];
    if (type === 0xda || type === 0xd9 || offset + 2 > bytes.length) return;
    const length = data.getUint16(offset);
    if (length < 2 || offset + length > bytes.length) return;
    if (type >= 0xc0 && type <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(type) && length >= 8) {
      return [data.getUint16(offset + 5), data.getUint16(offset + 3)];
    }
    offset += length;
  }
}

function webp(bytes, data) {
  if (bytes.length < 20 || data.getUint32(0) !== 0x52494646 || data.getUint32(8) !== 0x57454250) return;
  const end = data.getUint32(4, true) + 8;
  if (end > bytes.length) return;
  let canvas, image;
  const uint24 = offset => bytes[offset] + bytes[offset + 1] * 256 + bytes[offset + 2] * 65536;
  for (let offset = 12; offset + 8 <= end;) {
    const type = data.getUint32(offset), length = data.getUint32(offset + 4, true), start = offset + 8;
    if (start + length > end || type === 0x414e494d || type === 0x414e4d46) return; // ANIM/ANMF
    if (type === 0x56503858 && length >= 10) {
      if (canvas || bytes[start] & 2) return;
      canvas = [uint24(start + 4) + 1, uint24(start + 7) + 1];
    } else if (type === 0x56503820 && length >= 10) {
      if (image || bytes[start + 3] !== 0x9d || bytes[start + 4] !== 1 || bytes[start + 5] !== 0x2a) return;
      image = [data.getUint16(start + 6, true) & 0x3fff, data.getUint16(start + 8, true) & 0x3fff];
    } else if (type === 0x5650384c && length >= 5) {
      if (image || bytes[start] !== 0x2f) return;
      const size = data.getUint32(start + 1, true);
      image = [(size & 0x3fff) + 1, ((size >>> 14) & 0x3fff) + 1];
    }
    offset = start + length + (length % 2);
  }
  if (!image || (canvas && (canvas[0] !== image[0] || canvas[1] !== image[1]))) return;
  return image;
}

const decoders = { png, jpeg, webp };
const maxDimension = 4096, maxImagePixels = 4 * 1024 * 1024;
export const maxCardImagePixels = 8 * 1024 * 1024;

export function rasterPixels(source) {
  if (source.length > 2 * 1024 * 1024) return 0;
  const match = /^data:image\/(png|jpeg|webp);base64,([A-Za-z0-9+/]+=*)$/u.exec(source);
  if (!match) return 0;
  try {
    const bytes = Uint8Array.from(atob(match[2]), char => char.charCodeAt(0));
    const dimensions = decoders[match[1]](bytes, new DataView(bytes.buffer));
    if (!dimensions) return 0;
    const [width, height] = dimensions;
    if (width > 0 && height > 0 && width <= maxDimension && height <= maxDimension && width * height <= maxImagePixels) {
      return width * height;
    }
  } catch { /* Invalid data URL. */ }
  return 0;
}
