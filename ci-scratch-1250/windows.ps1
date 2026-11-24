# #1250 scratch: xperf with the Flint.Player provider beside the kernel's
# samples (the runner is elevated), then samply import --unknown-event-markers.
New-Item -ItemType Directory -Force out | Out-Null
$xperf = (Get-Command xperf -ErrorAction SilentlyContinue).Source
if (-not $xperf) {
  $xperf = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Windows Performance Toolkit\xperf.exe' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
}
"xperf: $xperf" | Tee-Object out/notes.txt
# WARP's frames run ~80 ms: a lower factor and floor, so its 100 ms stalls
# count (the detection rule is not what this run tests)
$env:FLINT_SHADOWS = "0"; $env:FLINT_HITCH_FACTOR = "1.15"; $env:FLINT_HITCH_FLOOR_MS = "8"
$guid = '89d78a2a-cb1c-507a-4e4b-896621b7b194'

function Wait-For($file, $pattern, $secs) {
  for ($i = 0; $i -lt $secs; $i++) {
    if ((Test-Path $file) -and (Select-String -Path $file -Pattern $pattern -Quiet)) { return $true }
    Start-Sleep 1
  }
  return $false
}

& $xperf -on PROC_THREAD+LOADER+PROFILE -stackwalk Profile -BufferSize 1024 -f kernel.etl -start flint -on $guid -BufferSize 1024 -f user.etl *> out/xperf-start.log
"xperf start exit $LASTEXITCODE" | Tee-Object -Append out/notes.txt
& target\profiling\flint-player.exe flint-examples\hitch_metronome.scene.toml --exit-after 20 --width 320 --height 180 --hitch-log "$PWD\out\hitches.jsonl" *> out/player.log
"player exit $LASTEXITCODE" | Tee-Object -Append out/notes.txt
& $xperf -stop flint -stop -d trace.etl *> out/xperf-stop.log
"xperf stop exit $LASTEXITCODE" | Tee-Object -Append out/notes.txt
Get-ChildItem *.etl | Format-Table | Out-String | Add-Content out/notes.txt

# What WPA would list as the provider's events
tracerpt user.etl -o out\user-events.xml -of XML -y *> out/tracerpt.log

samply import --unknown-event-markers --save-only -o profile.json.gz trace.etl *> out/samply-import.log
"samply import exit $LASTEXITCODE" | Tee-Object -Append out/notes.txt
Copy-Item profile.json.gz out/samply-profile.json.gz -ErrorAction SilentlyContinue

$load = Start-Process -FilePath samply -PassThru -NoNewWindow -ArgumentList 'load', '--no-open', 'profile.json.gz' `
  -RedirectStandardOutput out/samply-url.txt -RedirectStandardError out/samply-load.log
Wait-For out/samply-url.txt 'profiler.firefox.com' 60 | Out-Null
$margs = (python ci-scratch-1250/marks.py profile.json.gz out/samply-url.txt out/hitches.jsonl out/marks-windows.json 2> out/marks.log | Select-Object -Last 1)
"marks: $margs" | Tee-Object -Append out/notes.txt
$url = (Select-String -Path out/samply-url.txt -Pattern "https://profiler.firefox.com\S*").Matches[0].Value
$chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe"
$a = $margs -split " "
node ci-scratch-1250/profshot.mjs $url out/windows $chrome $a[0] $a[1] $a[2] *> out/profshot.log
Stop-Process -Id $load.Id -Force -ErrorAction SilentlyContinue
exit 0
