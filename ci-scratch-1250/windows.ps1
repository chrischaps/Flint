# #1250 scratch: xperf with the Flint.Player provider beside the kernel's
# samples (the runner is elevated), then samply import --unknown-event-markers.
New-Item -ItemType Directory -Force out | Out-Null
$xperf = (Get-Command xperf -ErrorAction SilentlyContinue).Source
if (-not $xperf) {
  $xperf = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Windows Performance Toolkit\xperf.exe' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
}
"xperf: $xperf" | Tee-Object out/notes.txt
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
& target\profiling\flint-player.exe flint-examples\hitch_metronome.scene.toml --exit-after 12 --hitch-log "$PWD\out\hitches.jsonl" *> out/player.log
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
python ci-scratch-1250/marks.py profile.json.gz out/samply-url.txt out/hitches.jsonl out/marks-windows.json *> out/marks.log
"marks exit $LASTEXITCODE" | Tee-Object -Append out/notes.txt
Stop-Process -Id $load.Id -Force -ErrorAction SilentlyContinue
exit 0
