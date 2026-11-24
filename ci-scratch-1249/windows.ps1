# #1249 scratch: the book's elevated-Windows samply command, as written, on a
# runner (hosted Windows runners run as Administrator).
New-Item -ItemType Directory -Force out | Out-Null
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
"administrator: $isAdmin" | Tee-Object out/elevation.txt
"examples\hitch_metronome.scene.toml exists: $(Test-Path examples\hitch_metronome.scene.toml)" | Tee-Object -Append out/elevation.txt
"flint-examples\hitch_metronome.scene.toml exists: $(Test-Path flint-examples\hitch_metronome.scene.toml)" | Tee-Object -Append out/elevation.txt

function Wait-For($file, $pattern, $secs) {
  for ($i = 0; $i -lt $secs; $i++) {
    if ((Test-Path $file) -and (Select-String -Path $file -Pattern $pattern -Quiet)) { return $true }
    Start-Sleep 1
  }
  return $false
}

# The book's command (with the scene's real folder, flint-examples\).
Remove-Item -ErrorAction SilentlyContinue profile.json.gz
$rec = Start-Process -FilePath samply -PassThru -NoNewWindow `
  -ArgumentList 'record', 'target\profiling\flint-player.exe', 'flint-examples\hitch_metronome.scene.toml', '--exit-after', '12', '--hitch-log', 'hitches.jsonl' `
  -RedirectStandardOutput out/samply-record.out -RedirectStandardError out/samply-record.log
$ok = Wait-For out/samply-record.log 'Local server listening' 240
"server up: $ok after the run; process exited: $($rec.HasExited)" | Tee-Object -Append out/notes.txt
Get-Process flint-player -ErrorAction SilentlyContinue | Format-Table | Out-String | Add-Content out/notes.txt
Stop-Process -Id $rec.Id -Force -ErrorAction SilentlyContinue
Get-Process samply -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process flint-player -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 2
Get-ChildItem profile.json.gz, hitches.jsonl -ErrorAction SilentlyContinue | Format-Table | Out-String | Add-Content out/notes.txt
Copy-Item hitches.jsonl out/ -ErrorAction SilentlyContinue
Copy-Item profile.json.gz out/samply-profile.json.gz -ErrorAction SilentlyContinue

$load = Start-Process -FilePath samply -PassThru -NoNewWindow -ArgumentList 'load', '--no-open', 'profile.json.gz' `
  -RedirectStandardOutput out/samply-url.txt -RedirectStandardError out/samply-load.log
Wait-For out/samply-url.txt 'profiler.firefox.com' 60 | Out-Null
$url = (Select-String -Path out/samply-url.txt -Pattern 'https://profiler.firefox.com\S*').Matches[0].Value
$chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
$th = (python ci-scratch-1249/symstats.py profile.json.gz out/samply-url.txt out/samply-windows-symstats.json 2> out/symstats.log | Select-Object -Last 1)
node ci-scratch-1249/profshot.mjs $url out/samply-windows $chrome $th *> out/profshot.log
Stop-Process -Id $load.Id -Force -ErrorAction SilentlyContinue
Get-ChildItem out | Format-Table
exit 0
