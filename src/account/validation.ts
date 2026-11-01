/* eslint-disable no-bitwise -- Standard base64 and UTF-8 encoding of public protocol fields. */
const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

export function validateUsername(value: string): boolean {
  const count = Array.from(value).length;
  return count >= 1 && count <= 32 &&
    !/[\p{Cc}\p{Cf}\p{Cs}]/u.test(value) &&
    !/^\p{White_Space}|\p{White_Space}$/u.test(value);
}

/** Canonical standard base64, including padding bits. Public material only. */
export function decodeBase64(value: unknown): number[] {
  if (typeof value !== 'string' || value.length > 4096 || value.length % 4 !== 0 ||
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new Error('Invalid base64.');
  }
  const result: number[] = [];
  for (let index = 0; index < value.length; index += 4) {
    const a = alphabet.indexOf(value[index]);
    const b = alphabet.indexOf(value[index + 1]);
    const c = alphabet.indexOf(value[index + 2]);
    const d = alphabet.indexOf(value[index + 3]);
    result.push((a << 2) | (b >> 4));
    if (c < 0) {
      if ((b & 15) !== 0) { throw new Error('Invalid base64.'); }
    } else {
      result.push(((b & 15) << 4) | (c >> 2));
      if (d < 0) {
        if ((c & 3) !== 0) { throw new Error('Invalid base64.'); }
      } else { result.push(((c & 3) << 6) | d); }
    }
  }
  return result;
}

export function usernameBase64(value: string): string {
  const bytes: number[] = [];
  for (const character of value) {
    const point = character.codePointAt(0)!;
    if (point < 0x80) { bytes.push(point); }
    else if (point < 0x800) { bytes.push(0xc0 | (point >> 6), 0x80 | (point & 63)); }
    else if (point < 0x10000) { bytes.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63)); }
    else { bytes.push(0xf0 | (point >> 18), 0x80 | ((point >> 12) & 63), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63)); }
  }
  let result = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const a = bytes[index]; const b = bytes[index + 1]; const c = bytes[index + 2];
    result += alphabet[a >> 2] + alphabet[((a & 3) << 4) | ((b ?? 0) >> 4)] +
      (b === undefined ? '=' : alphabet[((b & 15) << 2) | ((c ?? 0) >> 6)]) +
      (c === undefined ? '=' : alphabet[c & 63]);
  }
  return result;
}

export function isTimestamp(value: unknown): value is string {
  if (typeof value !== 'string') { return false; }
  const parts = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(?:Z|([+-])(\d{2}):(\d{2}))$/.exec(value);
  if (!parts) { return false; }
  const year = Number(parts[1]); const month = Number(parts[2]); const day = Number(parts[3]);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1] &&
    Number(parts[4]) < 24 && Number(parts[5]) < 60 && Number(parts[6]) < 60 &&
    (parts[7] === undefined || (Number(parts[8]) < 24 && Number(parts[9]) < 60)) &&
    Number.isFinite(Date.parse(value)) && Date.parse(value) >= 0;
}

export function isChallengeToken(value: unknown): value is string {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/.test(value);
}

export function isVoidId(value: unknown): value is string {
  return typeof value === 'string' && /^[1-9][0-9]{0,18}$/.test(value) &&
    (value.length < 19 || value <= '9223372036854775807');
}
