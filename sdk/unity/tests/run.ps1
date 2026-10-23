# Автотест C#-SDK Modly (sdk\unity\Modly.cs): тест сам поднимает канал вместо Modly и проверяет протокол.
# Использование:  powershell -ExecutionPolicy Bypass -File sdk\unity\tests\run.ps1
# Нужен .NET SDK 10+ (https://dotnet.microsoft.com). Код выхода 0 — всё прошло.
$ErrorActionPreference = 'Stop'

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    throw 'Не найден dotnet. Установите .NET SDK 10+ с https://dotnet.microsoft.com'
}
& dotnet run --project (Join-Path $PSScriptRoot 'ModlyTests.csproj') -c Release
exit $LASTEXITCODE
