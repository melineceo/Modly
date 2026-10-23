# Собирает мод компилятором из .NET SDK прямо против DLL игры (её папка Managed).
#   powershell -ExecutionPolicy Bypass -File build.ps1 -GamePath "C:\Games\YourGame"
# Нужен .NET SDK 6+ (https://dotnet.microsoft.com).
param(
    [Parameter(Mandatory = $true)][string]$GamePath,
    [string]$Name = 'ExampleMod'
)
$ErrorActionPreference = 'Stop'

$data = Get-ChildItem $GamePath -Directory -Filter '*_Data' | Select-Object -First 1
if (-not $data -or -not (Test-Path (Join-Path $data.FullName 'Managed\Assembly-CSharp.dll'))) {
    throw "Не похоже на Unity-игру на Mono: $GamePath"
}
$managed = Join-Path $data.FullName 'Managed'

$csc = Get-ChildItem "$env:ProgramFiles\dotnet\sdk\*\Roslyn\bincore\csc.dll" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $csc) {
    throw 'Не найден .NET SDK (csc.dll)'
}

$out = Join-Path $PSScriptRoot "bin\$Name.dll"
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null

$sdk = Join-Path $PSScriptRoot '..\..\sdk\unity\Modly.cs'
# Ссылки — только на сборки самой игры, без других модов: исходный список Unity (*.orig сохраняют
# установщики модов до того, как дописать в него свои DLL).
$list = Join-Path $data.FullName 'ScriptingAssemblies.json.orig'
if (-not (Test-Path $list)) {
    $list = Join-Path $data.FullName 'ScriptingAssemblies.json'
}
$gameDlls = (Get-Content -Raw -Encoding UTF8 $list | ConvertFrom-Json).names
$refs = Get-ChildItem $managed -Filter *.dll | Where-Object { $gameDlls -contains $_.Name -or $_.Name -eq 'mscorlib.dll' } |
    ForEach-Object { '-r:"' + $_.FullName + '"' }
$sources = @(Get-ChildItem (Join-Path $PSScriptRoot 'src') -Filter *.cs | ForEach-Object { '"' + $_.FullName + '"' }) + ('"' + (Resolve-Path $sdk).Path + '"')

$rsp = Join-Path $PSScriptRoot 'bin\build.rsp'
$lines = @('-nologo', '-nostdlib+', '-target:library', '-optimize+', '-deterministic', '-langversion:7.3',
    '-nowarn:CS1701,CS1702', ('-out:"' + $out + '"')) + $refs + $sources
[IO.File]::WriteAllLines($rsp, $lines, (New-Object Text.UTF8Encoding $false))

& dotnet $csc.FullName "@$rsp"
if ($LASTEXITCODE -ne 0) {
    throw "Сборка не удалась (код $LASTEXITCODE)"
}
Write-Host "OK: $out" -ForegroundColor Green
