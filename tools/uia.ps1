# Нажать элемент Modly по имени через Windows UI Automation (как экранный диктор) или вывести список.
#   powershell -ExecutionPolicy Bypass -File tools\uia.ps1 -Name 'Играть'
#   powershell -ExecutionPolicy Bypass -File tools\uia.ps1 -Action list
param([string]$Name = '', [string]$Action = 'invoke', [string]$Window = 'Modly', [int]$Index = 0)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$AE = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]
$win = $AE::RootElement.FindFirst($Scope::Children, (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $Window)))
if (-not $win) { 'no window'; exit 1 }
if ($Action -eq 'list') {
    $all = $win.FindAll($Scope::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($e in $all) { if ($e.Current.Name) { '{0} | {1}' -f $e.Current.ControlType.ProgrammaticName, $e.Current.Name } }
    exit
}
$found = $win.FindAll($Scope::Descendants, (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $Name)))
$n = 0
foreach ($el in $found) {
    if ($el.Current.ControlType.ProgrammaticName -eq 'ControlType.Text') { continue }
    if ($n -lt $Index) { $n++; continue }
    $p = $null
    if ($el.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$p)) { $p.Toggle(); "toggled: $Name -> $($p.Current.ToggleState)"; exit }
    if ($el.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$p)) { $p.Invoke(); "invoked: $Name"; exit }
    if ($el.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$p)) { $p.Select(); "selected: $Name"; exit }
    "no pattern on: $Name ($($el.Current.ControlType.ProgrammaticName))"; exit 1
}
"not found: $Name"; exit 1
