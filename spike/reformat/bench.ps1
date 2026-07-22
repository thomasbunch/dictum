# bench.ps1 - Dictum 0.3 reformatter spike harness.
# ponytail: throwaway. Drives llama-cli directly to get real latency + quality
# on THIS machine before we commit to the llama-cpp-2 subsystem. Not shipped.
#
# Runs the reformatter prompt over spike fixtures, once per mode (GPU/CPU),
# extracts the model's CLEANED output, and parses llama.cpp's perf timings.
#
# This script is pure ASCII on purpose (PS 5.1 reads un-BOM'd .ps1 as cp1252).
# The data files (system-prompt.txt, fixtures.jsonl) DO contain em-dashes and
# are read with -Encoding utf8 below so they reach the model intact.
#
# Usage (from spike/reformat/):
#   .\bench.ps1 -Model "C:\models\qwen2.5-3b-instruct-q4_k_m.gguf" -LlamaCli "C:\llama\llama-cli.exe"
#   .\bench.ps1 -Model "...\qwen2.5-1.5b-instruct-q4_k_m.gguf" -LlamaCli "..." -Modes CPU
# See README.md for where to get llama-cli (Vulkan build) and the GGUF models.

param(
  [Parameter(Mandatory = $true)][string]$Model,
  [Parameter(Mandatory = $true)][string]$LlamaCli,
  [ValidateSet('GPU', 'CPU', 'Both')][string]$Modes = 'Both',
  [int]$Ngl = 99,                       # GPU layers to offload for the GPU pass (0 = pure CPU)
  [int]$MaxTokens = 220,
  [int]$Ctx = 2048,
  [int]$Threads = [Environment]::ProcessorCount
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$sys = Get-Content -Raw -Encoding utf8 (Join-Path $here 'system-prompt.txt')
$fixtures = Get-Content -Encoding utf8 (Join-Path $here 'fixtures.jsonl') |
  Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json }

if (-not (Test-Path $Model))    { throw "Model not found: $Model  (see README.md)" }
if (-not (Test-Path $LlamaCli)) { throw "llama-cli not found: $LlamaCli  (see README.md)" }

$modeList = if ($Modes -eq 'Both') { @('GPU', 'CPU') } else { @($Modes) }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "dictum-spike-$stamp") -Force
$utf8NoBom = New-Object System.Text.UTF8Encoding $false

function Invoke-Reformat($promptText, $ngl) {
  $pf = Join-Path $tmp 'prompt.txt'
  $of = Join-Path $tmp 'out.txt'
  $ef = Join-Path $tmp 'err.txt'
  [System.IO.File]::WriteAllText($pf, $promptText, $utf8NoBom)   # UTF-8, no BOM, for llama-cli
  # Single quoted arg string is the most predictable way to pass paths (spaces-safe)
  # to a native exe via Start-Process on PS 5.1.
  $argStr = "-m `"$Model`" -f `"$pf`" -n $MaxTokens --temp 0 -ngl $ngl -c $Ctx -t $Threads -no-cnv --no-display-prompt"
  $p = Start-Process -FilePath $LlamaCli -ArgumentList $argStr -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput $of -RedirectStandardError $ef
  $out = if (Test-Path $of) { Get-Content -Raw -Encoding utf8 $of } else { '' }
  $err = if (Test-Path $ef) { Get-Content -Raw -Encoding utf8 $ef } else { '' }
  return [pscustomobject]@{ Out = $out; Err = $err; Exit = $p.ExitCode }
}

function Get-Cleaned($raw) {
  if (-not $raw) { return '' }
  # Robust to prompt-echo: take text after the LAST "CLEANED:", stop at the next "SPOKEN:".
  $after = ($raw -split 'CLEANED:')[-1]
  $clean = ($after -split 'SPOKEN:')[0]
  return $clean.Trim()
}

function Get-Metric($err, $pattern) {
  $m = [regex]::Match($err, $pattern)
  if ($m.Success) { return [double]$m.Groups[1].Value } else { return $null }
}

$allRows = @()
foreach ($mode in $modeList) {
  $ngl = if ($mode -eq 'GPU') { $Ngl } else { 0 }
  Write-Host ""
  Write-Host "===== $mode pass (ngl=$ngl, model=$(Split-Path -Leaf $Model)) =====" -ForegroundColor Cyan
  $rows = @()
  foreach ($fx in $fixtures) {
    $prompt = "$sys`nSPOKEN: $($fx.spoken)`nCLEANED:"
    $r = Invoke-Reformat $prompt $ngl
    $cleaned = Get-Cleaned $r.Out
    $totalMs = Get-Metric $r.Err 'total time\s*=\s*([\d.]+)\s*ms'
    $genTps  = Get-Metric $r.Err 'print:\s+eval time.*?([\d.]+)\s*tokens per second'
    $ttftMs  = Get-Metric $r.Err 'prompt eval time\s*=\s*([\d.]+)\s*ms'  # prefill ~= time-to-first-token
    $rows += [pscustomobject]@{
      Mode = $mode; Id = $fx.id; Held = $fx.heldout; Rule = $fx.rule
      TotalMs = $totalMs; GenTps = $genTps; PrefillMs = $ttftMs
      Cleaned = $cleaned; Expected = $fx.expected
    }
    $held = if ($fx.heldout) { 'held' } else { 'shot' }
    $tsec = if ($totalMs) { '{0:N1}s' -f ($totalMs / 1000) } else { '  ?  ' }
    $oneline = $cleaned -replace "`r?`n", ' / '
    Write-Host ("  #{0,-2} [{1}] {2,6}  {3}" -f $fx.id, $held, $tsec, $fx.rule)
    Write-Host ("       -> {0}" -f $oneline) -ForegroundColor Gray
  }
  $allRows += $rows

  # Summary. Held-out fixtures are the real quality signal; few-shot ids are a sanity check.
  $done = $rows | Where-Object { $_.TotalMs -ne $null }
  if ($done) {
    $avg = ($done | Measure-Object TotalMs -Average).Average
    $tpsAvg = ($done | Where-Object { $_.GenTps } | Measure-Object GenTps -Average).Average
    $preAvg = ($done | Where-Object { $_.PrefillMs } | Measure-Object PrefillMs -Average).Average
    Write-Host ("  -- {0}: avg end-to-end {1:N1}s | gen {2:N1} tok/s | prefill/TTFT {3:N0}ms --" -f `
        $mode, ($avg / 1000), $tpsAvg, $preAvg) -ForegroundColor Yellow
  } else {
    Write-Host ("  -- {0}: no timings parsed. Try dropping --no-display-prompt and/or -no-cnv (llama.cpp flag names drift). --" -f $mode) -ForegroundColor Red
  }
}

# Persist full outputs next to the script for eyeballing quality vs expected.
$results = Join-Path $here "results-$stamp.txt"
$sb = New-Object System.Text.StringBuilder
foreach ($row in $allRows) {
  $gotOne = $row.Cleaned -replace "`r?`n", ' / '
  $expOne = $row.Expected -replace "`r?`n", ' / '
  [void]$sb.AppendLine("### #$($row.Id) [$($row.Mode)] held=$($row.Held) - $($row.Rule)")
  [void]$sb.AppendLine("total=$($row.TotalMs)ms gen=$($row.GenTps)tok/s prefill=$($row.PrefillMs)ms")
  [void]$sb.AppendLine("GOT:      $gotOne")
  [void]$sb.AppendLine("EXPECTED: $expOne")
  [void]$sb.AppendLine("")
}
Set-Content -Path $results -Value $sb.ToString() -Encoding utf8
Write-Host ""
Write-Host "Full outputs -> $results" -ForegroundColor Green
Write-Host "Go/no-go: see README.md." -ForegroundColor Green
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
