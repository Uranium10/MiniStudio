// PCM WAV encoder header and interleaving tests.
import { describe, expect, it } from 'vitest'
import { encodeWav } from './wav'

describe('WAV encoder', () => {
  it('writes a stereo 16-bit PCM header and interleaved frames', () => {
    const wav = encodeWav([new Float32Array([1, -1]), new Float32Array([0.5, -0.5])], 48_000)
    const view = new DataView(wav.buffer)
    expect(new TextDecoder().decode(wav.slice(0, 4))).toBe('RIFF')
    expect(new TextDecoder().decode(wav.slice(8, 12))).toBe('WAVE')
    expect(view.getUint16(22, true)).toBe(2)
    expect(view.getUint32(24, true)).toBe(48_000)
    expect(view.getUint32(40, true)).toBe(8)
    expect(view.getInt16(44, true)).toBe(32767)
    expect(view.getInt16(46, true)).toBe(16383)
  })
})
