// Manual raw-binary IPC exception: little-endian f32 pairs [min, max, min, max, ...].
import { invoke } from '@tauri-apps/api/core'

export async function getAssetPeaks(assetId: string, lod: number): Promise<Float32Array> {
  const buffer = await invoke<ArrayBuffer>('plugin:binary|get_asset_peaks', { assetId, lod })
  return new Float32Array(buffer)
}
