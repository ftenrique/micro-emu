$ErrorActionPreference = "Stop"

trap {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

if ($null -eq $WindowHandle) {
    throw "$AgentName window handle was not supplied"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$descendants = [System.Windows.Automation.TreeScope]::Descendants
$allElements = [System.Windows.Automation.Condition]::TrueCondition

function Find-ComposerCandidates {
    param([System.Windows.Automation.AutomationElement]$Root)

    $candidates = @()
    $elements = $Root.FindAll($descendants, $allElements)
    for ($index = 0; $index -lt $elements.Count; $index++) {
        $element = $elements.Item($index)
        try {
            $current = $element.Current
            if (-not $current.IsEnabled -or -not $current.IsKeyboardFocusable) { continue }

            $name = if ($null -eq $current.Name) { "" } else { $current.Name }
            $controlType = $current.ControlType
            $isTextInput = $controlType -eq [System.Windows.Automation.ControlType]::Edit -or
                $controlType -eq [System.Windows.Automation.ControlType]::Document
            if (-not $isTextInput) { continue }

            # Search fields and sidebar filters are also Edit controls. They
            # must never win over the message composer.
            if ($name -match "(?i)search|filter|session|project|find") { continue }

            $rect = $current.BoundingRectangle
            if ($rect.Width -le 0 -or $rect.Height -le 0) { continue }

            $score = if ($controlType -eq [System.Windows.Automation.ControlType]::Edit) { 100 } else { 80 }
            if ($name -match "(?i)message|prompt|composer|chat|ask|reply|question") { $score += 40 }
            # When Chromium exposes only anonymous text controls, the composer
            # is normally the lowest usable input in the agent window.
            $score += [Math]::Min(30, [int]($rect.Bottom / 100))
            $candidates += [PSCustomObject]@{ Element = $element; Score = $score; Bottom = $rect.Bottom }
        }
        catch {
            # UIA elements can disappear while React/Chromium is rendering.
            continue
        }
    }
    return $candidates | Sort-Object Score, Bottom -Descending
}

function Test-SameElement {
    param(
        [System.Windows.Automation.AutomationElement]$First,
        [System.Windows.Automation.AutomationElement]$Second
    )

    try {
        $firstId = $First.GetRuntimeId()
        $secondId = $Second.GetRuntimeId()
        if ($firstId.Count -ne $secondId.Count) { return $false }
        for ($index = 0; $index -lt $firstId.Count; $index++) {
            if ($firstId[$index] -ne $secondId[$index]) { return $false }
        }
        return $true
    }
    catch {
        return $false
    }
}

$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($null -eq $root) {
    throw "$AgentName window is not available to Windows UI Automation"
}

$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.ElapsedMilliseconds -lt 2500) {
    $candidates = @(Find-ComposerCandidates $root)
    foreach ($candidate in $candidates) {
        try {
            $candidate.Element.SetFocus()
            Start-Sleep -Milliseconds 60
            $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
            if ($null -ne $focused -and (Test-SameElement $focused $candidate.Element)) {
                [Console]::Out.WriteLine("focused")
                exit 0
            }
        }
        catch {
            continue
        }
    }
    Start-Sleep -Milliseconds 150
}

throw "Could not focus the $AgentName message composer"
