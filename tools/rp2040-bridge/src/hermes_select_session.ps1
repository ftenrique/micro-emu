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
if ($null -eq $TargetSessionId -or [string]::IsNullOrWhiteSpace($TargetSessionId)) {
    throw "Hermes session id was not supplied"
}

# Remote-backed task cards can carry a backend-truncated preview as their
# title (a trailing ellipsis). Sidebar rows hold the full text, so the title
# search and row scoring also try the title without the truncation marker.
$SearchTitle = $TargetTitle.TrimEnd([char]0x2026).TrimEnd()

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

# Hermes renders every sidebar session row as a button whose class begins
# with the row's padding/gap utilities (pinned, project, and messaging
# sections share it). The exact prefix differs across Hermes builds — builds
# since 2026-08 dropped the pr-1 padding — so accept every known variant; a
# single literal would let an app update hide all rows from the automation.
function Test-SessionRow {
    param($element)

    if ($element.Current.ControlType -ne [System.Windows.Automation.ControlType]::Button) {
        return $false
    }
    $className = $element.Current.ClassName
    foreach ($prefix in @("pl-2 gap-1.5", "pl-2 pr-1 gap-1.5")) {
        if ($className.StartsWith($prefix, [System.StringComparison]::Ordinal)) {
            return $true
        }
    }
    return $false
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
    if ($SearchTitle -ne $TargetTitle -and $Name.Contains($SearchTitle)) { return 1 }
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

function Find-AnySessionRow {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        Test-SessionRow $element
    }
}

function Get-SessionRowCount {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    $count = 0
    $elements = $SearchRoot.FindAll($descendants, $allElements)
    for ($index = 0; $index -lt $elements.Count; $index++) {
        if (Test-SessionRow $elements.Item($index)) {
            $count++
        }
    }
    return $count
}

function Get-SessionSearchTerms {
    # The bridge normally passes the native id, but old/re-published task
    # snapshots can preserve one or more bridge namespaces. Hermes indexes the
    # native id, so try it first and then increasingly clean variants instead
    # of allowing a wrapper to make an otherwise visible task disappear.
    $terms = [System.Collections.Generic.List[string]]::new()
    $candidate = $TargetSessionId.Trim()
    while (-not [string]::IsNullOrWhiteSpace($candidate)) {
        if (-not $terms.Contains($candidate)) {
            $terms.Add($candidate)
        }
        if ($candidate.StartsWith("hermes:", [System.StringComparison]::OrdinalIgnoreCase)) {
            $candidate = $candidate.Substring("hermes:".Length).Trim()
        } else {
            break
        }
    }
    return $terms
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

# Warm Chromium's accessibility tree briefly. Selection below filters Hermes'
# own session list by the stable session id, so it does not need to scan a large
# sidebar or wait for a title that may be duplicated or stale.
$warmup = [System.Diagnostics.Stopwatch]::StartNew()
while ($true) {
    $searchBox = Find-SearchBox $root
    $toggle = Find-SidebarToggle $root
    $warmRow = Find-AnySessionRow $root
    if ($null -ne $searchBox -or $null -ne $toggle -or $null -ne $warmRow) {
        break
    }
    if ($warmup.ElapsedMilliseconds -ge 3000) {
        break
    }
    Invoke-AccessibilityPoke ([IntPtr]$WindowHandle)
    Start-Sleep -Milliseconds 150
}

# The session list lives in the collapsible sidebar. When the user keeps it
# collapsed the rows are not rendered at all, so open it for the selection
# and restore the collapsed state afterwards.
$sidebarOpened = $false
if ($null -eq $toggle) {
    $toggle = Find-SidebarToggle $root
}
if ($null -ne $toggle -and $toggle.Current.Name -eq "Show sidebar") {
    if (-not (Invoke-Element $toggle)) {
        throw "The Hermes sidebar toggle has no invoke pattern"
    }
    $sidebarOpened = $true
    $searchBox = Wait-For { Find-SearchBox $root } 2500
}

# Fast path: a visible session row can be invoked directly. This avoids
# filtering the full sidebar (and the extra Chromium render pass it causes)
# for the common case where the selected task is already on screen. The
# warm-up probe above must not feed this: it accepts any positional row, so
# it would click whatever happens to be first in the sidebar.
$sessionRow = Find-SessionRow $root

# Hermes' command-center and sidebar search both index the stable session id.
# Filtering by it makes duplicate titles unambiguous, and it is the only
# reliable query: the sidebar search also matches message content, so a title
# query can rank another session's excerpt above the target. Try cleaned id
# variants too: a bridged/re-published task can carry redundant `hermes:`
# wrappers that Hermes itself does not index. Fall back to title only for
# older Hermes builds that do not index ids in the sidebar search.
$searchUsed = $false
if ($null -eq $searchBox) {
    $searchBox = Find-SearchBox $root
}
if ($null -eq $sessionRow -and $null -ne $searchBox) {
    $searchUsed = $true
    foreach ($searchTerm in Get-SessionSearchTerms) {
        # Hermes resolves the filter through the session backend, so the row
        # list keeps showing the unfiltered sessions for a moment after the
        # text lands. Find-AnySessionRow would happily return one of those,
        # so only accept a row after the list has moved off its pre-search
        # row count (a miss settles on zero rows, a hit narrows the list).
        $baseline = Get-SessionRowCount $root
        if (-not (Set-SearchText $searchBox $searchTerm)) { break }
        $sessionRow = Wait-For {
            if ((Get-SessionRowCount $root) -eq $baseline) { return $null }
            Find-AnySessionRow $root
        } 3000
        if ($null -ne $sessionRow) { break }
    }
    if ($null -eq $sessionRow -and (Set-SearchText $searchBox $SearchTitle)) {
        Start-Sleep -Milliseconds 250
        $sessionRow = Wait-For { Find-SessionRow $root } 2000
    }
} elseif ($null -eq $sessionRow -and $null -eq $searchBox) {
    $sessionRow = Wait-For { Find-SessionRow $root } 2500
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
# tab label renders uppercased and its accessible name concatenates sibling
# content (draft badges, the tab Close button), so compare case-insensitively
# on containment rather than equality. Waiting here keeps racing observers
# (and users) from turning a slow switch into a false negative.
$activated = $false
$loweredTitle = $TargetTitle.ToLowerInvariant()
$loweredSearch = $SearchTitle.ToLowerInvariant()
$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.ElapsedMilliseconds -lt 2500) {
    $activeTab = Find-Element $root {
        param($element)
        if ($element.Current.ControlType -ne [System.Windows.Automation.ControlType]::TabItem) {
            return $false
        }
        if (-not $element.Current.ClassName.Contains("tab-active")) {
            return $false
        }
        $name = $element.Current.Name.ToLowerInvariant()
        return $name.Contains($loweredTitle) -or
            ($loweredSearch -ne $loweredTitle -and $name.Contains($loweredSearch))
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
