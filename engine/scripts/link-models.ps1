# Dev setup: hardlink the spike's downloaded weights from .models/ into .data/models/ using
# sd.cpp's category layout, so the engine sees them as installed without re-downloading.
# Hardlinks, so nothing is copied or moved. Files missing from
# .models/ are skipped (pull them with the CLI instead). The engine verifies every sha256.
# Usage: .\engine\scripts\link-models.ps1
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$src = "$root\.models"
$dst = "$root\.data\models"
# source (under .models) -> category\filename (under .data\models)
$links = [ordered]@{
  'zimage\z_image_turbo-Q4_K.gguf'            = 'diffusion_models\z_image_turbo-Q4_K.gguf'
  'zimage\Qwen3-4B-Instruct-2507-Q4_K_M.gguf' = 'text_encoders\Qwen3-4B-Instruct-2507-Q4_K_M.gguf'
  'zimage\ae.safetensors'                     = 'vae\ae.safetensors'
  'klein\flux-2-klein-4b-Q8_0.gguf'           = 'diffusion_models\flux-2-klein-4b-Q8_0.gguf'
  'klein\Qwen3-4B-Q4_K_M.gguf'                = 'text_encoders\Qwen3-4B-Q4_K_M.gguf'
  'klein\flux2-vae.safetensors'               = 'vae\flux2-vae.safetensors'
  'sdxl\RealVisXL_V5.0_fp16.safetensors'      = 'checkpoints\RealVisXL_V5.0_fp16.safetensors'
  'sdxl\sdxl.vae.safetensors'                 = 'vae\sdxl.vae.safetensors'
}
foreach ($from in $links.Keys) {
  $target = "$src\$from"
  $link = "$dst\$($links[$from])"
  if (-not (Test-Path $target)) { Write-Host "skip    $($links[$from]) (no $from)"; continue }
  if (Test-Path $link) { Write-Host "ok      $($links[$from])"; continue }
  New-Item -ItemType Directory -Force (Split-Path $link) | Out-Null
  New-Item -ItemType HardLink -Path $link -Target $target | Out-Null
  Write-Host "linked  $($links[$from])"
}
