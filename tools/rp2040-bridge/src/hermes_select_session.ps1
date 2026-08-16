$ErrorActionPreference = "Stop"

trap {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

if ($null -eq $WindowHandle) {
    throw "Hermes window handle was not supplied"
}
if ($null -eq $TargetTitle -or [string]::IsNullOrWhiteSpace($TargetTitle)) {
    throw "Hermes session title was not supplied"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

# Chromium builds its accessibility tree lazily: it only switches the tree on
# once it detects an assistive client. Plain UIA queries eventually trigger
# the detection, but poking MSAA (AccessibleObjectFromWindow on the render
# widget, falling back to the main window) makes the switch happen reliably
# on the first iterations instead of after an unbounded delay.
$msaaTap = @"
using System;
using System.Runtime.InteropServices;
public class HermesMsaaTap {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr child, string className, string title);
  [DllImport("oleacc.dll")]
  public static extern int AccessibleObjectFromWindow(IntPtr hwnd, uint id, ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object ppv);
}
"@
Add-Type -TypeDefinition $msaaTap

$descendants = [System.Windows.Automation.TreeScope]::Descendants
$allElements = [System.Windows.Automation.Condition]::TrueCondition
$invokePatternId = [System.Windows.Automation.InvokePattern]::Pattern
$valuePatternId = [System.Windows.Automation.ValuePattern]::Pattern

function Invoke-AccessibilityPoke {
    param([IntPtr]$Window)

    $target = [HermesMsaaTap]::FindWindowExW($Window, [IntPtr]::Zero, "Chrome_RenderWidgetHostHWND", $null)
    if ($target -eq [IntPtr]::Zero) {
        $target = $Window
    }
    $iid = [Guid]"618736e0-3c3d-11cf-810c-00aa00389b71"  # IAccessible
    $obj = $null
    $null = [HermesMsaaTap]::AccessibleObjectFromWindow($target, [uint32]4294967280, [ref]$iid, [ref]$obj)  # OBJID_CLIENT
    if ($null -ne $obj) {
        $null = [System.Runtime.InteropServices.Marshal]::ReleaseComObject($obj)
    }
}

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

function Find-Element {
    param(
        [System.Windows.Automation.AutomationElement]$SearchRoot,
        [scriptblock]$Predicate
    )

    $elements = $SearchRoot.FindAll($descendants, $allElements)
    for ($index = 0; $index -lt $elements.Count; $index++) {
        $element = $elements.Item($index)
        if (& $Predicate $element) {
            return $element
        }
    }
    return $null
}

# Hermes renders every sidebar session row as a button whose class starts
# with "pl-2 pr-1 gap-1.5" (pinned, recent, and messaging sections share it).
function Test-SessionRow {
    param($element)

    return $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
        $element.Current.ClassName.StartsWith("pl-2 pr-1 gap-1.5", [System.StringComparison]::Ordinal)
}

# Row names differ per section: pinned and messaging rows are exactly the
# title, while recent rows concatenate their drag handle ("Reorder <title>")
# with the title text. Score so exact matches beat substring matches and
# titles that are prefixes of each other never collide.
function Get-RowScore {
    param([string]$Name)

    if ($Name -ceq $TargetTitle) { return 3 }
    if ($Name.StartsWith("Reorder " + $TargetTitle, [System.StringComparison]::Ordinal)) { return 2 }
    if ($Name.Contains($TargetTitle)) { return 1 }
    return 0
}

function Find-SessionRow {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    $best = $null
    $bestScore = 0
    $elements = $SearchRoot.FindAll($descendants, $allElements)
    for ($index = 0; $index -lt $elements.Count; $index++) {
        $element = $elements.Item($index)
        if (-not (Test-SessionRow $element)) { continue }
        $score = Get-RowScore $element.Current.Name
        if ($score -gt $bestScore) {
            $best = $element
            $bestScore = $score
        }
    }
    return $best
}

function Find-SidebarToggle {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
            ($element.Current.Name -eq "Show sidebar" -or $element.Current.Name -eq "Hide sidebar")
    }
}

function Find-SearchBox {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit -and
            $element.Current.Name -eq "Search sessions"
    }
}

function Invoke-Element {
    param([System.Windows.Automation.AutomationElement]$Element)

    $invokePattern = Get-Pattern $Element $invokePatternId
    if ($null -ne $invokePattern) {
        $invokePattern.Invoke()
        return $true
    }
    return $false
}

function Wait-For {
    param(
        [scriptblock]$Probe,
        [int]$TimeoutMilliseconds
    )

    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $found = & $Probe
        if ($null -ne $found) {
            return $found
        }
        Start-Sleep -Milliseconds 150
    } while ($timer.ElapsedMilliseconds -lt $TimeoutMilliseconds)
    return $null
}

function Set-SearchText {
    param(
        [System.Windows.Automation.AutomationElement]$SearchBox,
        [string]$Text
    )

    $valuePattern = Get-Pattern $SearchBox $valuePatternId
    if ($null -eq $valuePattern) {
        return $false
    }
    $valuePattern.SetValue($Text)
    return $true
}

$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($null -eq $root) {
    throw "Hermes window is not available to Windows UI Automation"
}

# Keep polling until the session list materializes. A Hermes that has been
# running for a while without any automation client tears its accessibility
# tree down, and rebuilding it for a large session list can take many seconds,
# so the budget here is deliberately generous.
$sessionRow = $null
$warmup = [System.Diagnostics.Stopwatch]::StartNew()
while ($null -eq $sessionRow) {
    $sessionRow = Find-SessionRow $root
    if ($null -ne $sessionRow) {
        break
    }
    if ($warmup.ElapsedMilliseconds -ge 20000) {
        break
    }
    Invoke-AccessibilityPoke ([IntPtr]$WindowHandle)
    Start-Sleep -Milliseconds 250
}

# The session list lives in the collapsible sidebar. When the user keeps it
# collapsed the rows are not rendered at all, so open it for the selection
# and restore the collapsed state afterwards.
$sidebarOpened = $false
if ($null -eq $sessionRow) {
    $toggle = Find-SidebarToggle $root
    if ($null -ne $toggle -and $toggle.Current.Name -eq "Show sidebar") {
        if (-not (Invoke-Element $toggle)) {
            throw "The Hermes sidebar toggle has no invoke pattern"
        }
        $sidebarOpened = $true
        $sessionRow = Wait-For { Find-SessionRow $root } 4000
    }
}

# Older sessions sit behind the sidebar's initial page. The search box
# filters the whole list, so typing the title surfaces any row; the filter
# is cleared afterwards to leave the user's view unchanged.
$searchUsed = $false
if ($null -eq $sessionRow) {
    $searchBox = Find-SearchBox $root
    if ($null -ne $searchBox -and (Set-SearchText $searchBox $TargetTitle)) {
        $searchUsed = $true
        $sessionRow = Wait-For { Find-SessionRow $root } 4000
    }
}

if ($null -eq $sessionRow) {
    if ($searchUsed) {
        $searchBox = Find-SearchBox $root
        if ($null -ne $searchBox) {
            $null = Set-SearchText $searchBox ""
        }
    }
    if ($sidebarOpened) {
        $toggle = Find-SidebarToggle $root
        if ($null -ne $toggle -and $toggle.Current.Name -eq "Hide sidebar") {
            $null = Invoke-Element $toggle
        }
    }
    throw "Could not find a Hermes session titled '$TargetTitle'"
}

if (-not (Invoke-Element $sessionRow)) {
    throw "The Hermes session row for '$TargetTitle' has no invoke pattern"
}

if ($searchUsed) {
    Start-Sleep -Milliseconds 400
    $searchBox = Find-SearchBox $root
    if ($null -ne $searchBox) {
        $null = Set-SearchText $searchBox ""
    }
}

# Confirm the app actually activated the session: open sessions render as
# editor tabs, and the active tab carries the tab-active styling class. The
# tab label renders uppercased, so compare case-insensitively. Waiting here
# keeps racing observers (and users) from turning a slow switch into a false
# negative.
$activated = $false
$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.ElapsedMilliseconds -lt 5000) {
    $activeTab = Find-Element $root {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem -and
            $element.Current.Name -eq $TargetTitle -and
            $element.Current.ClassName.Contains("tab-active")
    }
    if ($null -ne $activeTab) {
        $activated = $true
        break
    }
    Start-Sleep -Milliseconds 200
}

if ($sidebarOpened) {
    Start-Sleep -Milliseconds 400
    $toggle = Find-SidebarToggle $root
    if ($null -ne $toggle -and $toggle.Current.Name -eq "Hide sidebar") {
        $null = Invoke-Element $toggle
    }
}

if (-not $activated) {
    throw "The Hermes session '$TargetTitle' did not activate after selection"
}

[Console]::Out.WriteLine("selected")
