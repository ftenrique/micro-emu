$ErrorActionPreference = "Stop"

trap {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

if ($null -eq $WindowHandle) {
    throw "Codex window handle was not supplied"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$descendants = [System.Windows.Automation.TreeScope]::Descendants
$allElements = [System.Windows.Automation.Condition]::TrueCondition
$expandPatternId = [System.Windows.Automation.ExpandCollapsePattern]::Pattern
$invokePatternId = [System.Windows.Automation.InvokePattern]::Pattern

function Get-Pattern {
    param(
        [System.Windows.Automation.AutomationElement]$Element,
        [System.Windows.Automation.AutomationPattern]$Pattern
    )

    try {
        return $Element.GetCurrentPattern($Pattern)
    }
    catch {
        return $null
    }
}

function Wait-ForElement {
    param(
        [System.Windows.Automation.AutomationElement]$SearchRoot,
        [scriptblock]$Predicate,
        [int]$TimeoutMilliseconds = 2500
    )

    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $elements = $SearchRoot.FindAll($descendants, $allElements)
        for ($index = 0; $index -lt $elements.Count; $index++) {
            $element = $elements.Item($index)
            if (& $Predicate $element) {
                return $element
            }
        }
        Start-Sleep -Milliseconds 50
    } while ($timer.ElapsedMilliseconds -lt $TimeoutMilliseconds)

    return $null
}

function Test-VisibleEnabledElement {
    param([System.Windows.Automation.AutomationElement]$Element)

    return $Element.Current.IsEnabled -and -not $Element.Current.IsOffscreen
}

$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($null -eq $root) {
    throw "Codex window is not available to Windows UI Automation"
}

$modelButton = Wait-ForElement $root {
    param($element)
    (Test-VisibleEnabledElement $element) -and
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
        $element.Current.Name -match '(?i)^\s*(?:gpt-)?5\.6\s+(Sol|Terra|Luna)\b' -and
        $null -ne (Get-Pattern $element $expandPatternId)
}
if ($null -eq $modelButton) {
    throw "Could not find the active Codex model control"
}

$script:modelButtonName = $modelButton.Current.Name
$currentMatch = [regex]::Match($modelButton.Current.Name, '(?i)\b(Sol|Terra|Luna)\b')
if (-not $currentMatch.Success) {
    throw "Could not read the current model from '$($modelButton.Current.Name)'"
}

$currentName = $currentMatch.Groups[1].Value.ToLowerInvariant()
$script:currentName = $currentName
$targetName = switch ($currentName) {
    "sol" { "Terra" }
    "terra" { "Luna" }
    "luna" { "Sol" }
    default { throw "Unsupported current model '$currentName'" }
}
$script:targetName = $targetName
$targetId = "gpt-5.6-$($targetName.ToLowerInvariant())"

$mainPattern = Get-Pattern $modelButton $expandPatternId
$advancedPattern = $null
$modelPattern = $null
$selected = $false

try {
    if ($mainPattern.Current.ExpandCollapseState -ne [System.Windows.Automation.ExpandCollapseState]::Expanded) {
        $mainPattern.Expand()
    }

    $modelMenu = Wait-ForElement $root {
        param($element)
        (Test-VisibleEnabledElement $element) -and
            $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Menu -and
            $element.Current.Name -eq $script:modelButtonName
    }
    if ($null -eq $modelMenu) {
        throw "Codex model menu did not open"
    }

    # Advanced mode already exposes separate Model, Effort, and Speed entries.
    # Check for the Model entry first because Codex remembers this view between
    # openings. Its proper name (Sol/Terra/Luna) stays stable across locales.
    $modelItem = Wait-ForElement $root {
        param($element)
        (Test-VisibleEnabledElement $element) -and
            $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::MenuItem -and
            $element.Current.Name -match "(?i)\b$script:currentName\b" -and
            $null -ne (Get-Pattern $element $expandPatternId)
    } 200

    if ($null -eq $modelItem) {
        # Compact mode has one expandable menu item: the switch to advanced
        # options. Scope it to this model menu so application menus are ignored.
        $advancedItem = Wait-ForElement $modelMenu {
            param($element)
            (Test-VisibleEnabledElement $element) -and
                $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::MenuItem -and
                $null -ne (Get-Pattern $element $expandPatternId)
        }
        if ($null -eq $advancedItem) {
            throw "Could not open Codex model options"
        }
        $advancedPattern = Get-Pattern $advancedItem $expandPatternId
        if ($advancedPattern.Current.ExpandCollapseState -ne [System.Windows.Automation.ExpandCollapseState]::Expanded) {
            $advancedPattern.Expand()
        }

        $modelItem = Wait-ForElement $root {
            param($element)
            (Test-VisibleEnabledElement $element) -and
                $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::MenuItem -and
                $element.Current.Name -match "(?i)\b$script:currentName\b" -and
                $null -ne (Get-Pattern $element $expandPatternId)
        }
    }
    if ($null -eq $modelItem) {
        throw "Could not find the model submenu"
    }
    $modelPattern = Get-Pattern $modelItem $expandPatternId
    $modelPattern.Expand()

    $targetItem = Wait-ForElement $root {
        param($element)
        (Test-VisibleEnabledElement $element) -and
            $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::MenuItem -and
            $element.Current.Name -match "(?i)^\s*(?:gpt-)?5\.6\s+$script:targetName\s*$" -and
            $null -ne (Get-Pattern $element $invokePatternId)
    }
    if ($null -eq $targetItem) {
        throw "Could not find target model 5.6 $targetName"
    }

    $invokePattern = Get-Pattern $targetItem $invokePatternId
    $invokePattern.Invoke()

    # Do not acknowledge the hardware action until Codex exposes the new model
    # on its composer button. This avoids the old flash-without-change failure.
    $confirmedButton = Wait-ForElement $root {
        param($element)
        (Test-VisibleEnabledElement $element) -and
            $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
            $element.Current.Name -match "(?i)^\s*(?:gpt-)?5\.6\s+$script:targetName\b" -and
            $null -ne (Get-Pattern $element $expandPatternId)
    } 3500
    if ($null -eq $confirmedButton) {
        throw "Codex did not confirm model 5.6 $targetName"
    }

    $selected = $true
}
finally {
    if (-not $selected) {
        foreach ($pattern in @($modelPattern, $advancedPattern, $mainPattern)) {
            if ($null -ne $pattern) {
                try { $pattern.Collapse() } catch { }
            }
        }
    }
}

[Console]::Out.WriteLine($targetId)
