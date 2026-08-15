export type HspColor = { h: number; s: number; p: number }

const RED_WEIGHT = .299
const GREEN_WEIGHT = .587
const BLUE_WEIGHT = .114

export function perceivedBrightness(red: number, green: number, blue: number): number {
  return Math.sqrt(RED_WEIGHT * red ** 2 + GREEN_WEIGHT * green ** 2 + BLUE_WEIGHT * blue ** 2)
}

export function hexToHsp(hex: string): HspColor {
  const match = /^#?([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i.exec(hex)
  if (!match) return { h: 0, s: 0, p: 50 }
  const red = parseInt(match[1]!, 16) / 255
  const green = parseInt(match[2]!, 16) / 255
  const blue = parseInt(match[3]!, 16) / 255
  const { h, s } = rgbToHsl(red, green, blue)
  return { h: Math.round(h * 360), s: Math.round(s * 100), p: Math.round(perceivedBrightness(red, green, blue) * 100) }
}

/**
 * Converts hue/saturation/perceived-brightness to an in-gamut RGB colour.
 * HSL lightness is found by binary search so the resulting RGB colour matches
 * the requested HSP brightness without flattening saturated hues by clipping.
 */
export function hspToHex({ h, s, p }: HspColor): string {
  const hue = ((h % 360) + 360) % 360 / 360
  const saturation = clip(s / 100)
  const target = clip(p / 100)
  let low = 0
  let high = 1
  for (let index = 0; index < 18; index += 1) {
    const lightness = (low + high) / 2
    const [red, green, blue] = hslToRgb(hue, saturation, lightness)
    if (perceivedBrightness(red, green, blue) < target) low = lightness
    else high = lightness
  }
  const [red, green, blue] = hslToRgb(hue, saturation, (low + high) / 2)
  return `#${[red, green, blue].map((channel) => Math.round(clip(channel) * 255).toString(16).padStart(2, '0')).join('')}`
}

function rgbToHsl(red: number, green: number, blue: number): { h: number; s: number; l: number } {
  const max = Math.max(red, green, blue)
  const min = Math.min(red, green, blue)
  const lightness = (max + min) / 2
  if (max === min) return { h: 0, s: 0, l: lightness }
  const delta = max - min
  const saturation = lightness > .5 ? delta / (2 - max - min) : delta / (max + min)
  const hue = max === red
    ? ((green - blue) / delta + (green < blue ? 6 : 0)) / 6
    : max === green
      ? ((blue - red) / delta + 2) / 6
      : ((red - green) / delta + 4) / 6
  return { h: hue, s: saturation, l: lightness }
}

function hslToRgb(hue: number, saturation: number, lightness: number): [number, number, number] {
  if (saturation === 0) return [lightness, lightness, lightness]
  const q = lightness < .5 ? lightness * (1 + saturation) : lightness + saturation - lightness * saturation
  const p = 2 * lightness - q
  return [hueChannel(p, q, hue + 1 / 3), hueChannel(p, q, hue), hueChannel(p, q, hue - 1 / 3)]
}

function hueChannel(p: number, q: number, source: number): number {
  let value = source
  if (value < 0) value += 1
  if (value > 1) value -= 1
  if (value < 1 / 6) return p + (q - p) * 6 * value
  if (value < 1 / 2) return q
  if (value < 2 / 3) return p + (q - p) * (2 / 3 - value) * 6
  return p
}

function clip(value: number): number { return Math.max(0, Math.min(1, value)) }
