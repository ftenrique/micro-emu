$ErrorActionPreference = "Stop"

trap {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

if ($null -eq $WindowHandle) {
    throw "ZCode window handle was not supplied"
}
if ($null -eq $TargetTitle -or [string]::IsNullOrWhiteSpace($TargetTitle)) {
    throw "ZCode session title was not supplied"
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
public class ZCodeMsaaTap {
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
$selectionPatternId = [System.Windows.Automation.SelectionItemPattern]::Pattern

function Invoke-AccessibilityPoke {
    param([IntPtr]$Window)

    $target = [ZCodeMsaaTap]::FindWindowExW($Window, [IntPtr]::Zero, "Chrome_RenderWidgetHostHWND", $null)
    if ($target -eq [IntPtr]::Zero) {
        $target = $Window
    }
    $iid = [Guid]"618736e0-3c3d-11cf-810c-00aa00389b71"  # IAccessible
    $obj = $null
    $null = [ZCodeMsaaTap]::AccessibleObjectFromWindow($target, 0xFFFFFFF0, [ref]$iid, [ref]$obj)  # OBJID_CLIENT
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

# Session rows expose "<title><relative time>" as their accessible name (for
# example "Refactor auth4h"), so a prefix match also distinguishes rows whose
# titles are prefixes of each other.
function Find-SessionItem {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::ListItem -and
            $element.Current.Name.StartsWith($TargetTitle, [System.StringComparison]::Ordinal)
    }
}

function Find-ToggleSidebar {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
            $element.Current.Name -eq "Toggle sidebar"
    }
}

function Find-ShowMore {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::ListItem -and
            $element.Current.Name -eq "Show more"
    }
}

function Invoke-Element {
    param([System.Windows.Automation.AutomationElement]$Element)

    $invokePattern = Get-Pattern $Element $invokePatternId
    if ($null -ne $invokePattern) {
        $invokePattern.Invoke()
        return $true
    }
    $selectionPattern = Get-Pattern $Element $selectionPatternId
    if ($null -ne $selectionPattern) {
        $selectionPattern.Select()
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

$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($null -eq $root) {
    throw "ZCode window is not available to Windows UI Automation"
}

# Keep polling until the session list materializes. A ZCode that has been
# running for a while without any automation client tears its accessibility
# tree down, and rebuilding it for a large conversation can take many seconds,
# so the budget here is deliberately generous.
$sessionItem = $null
$warmup = [System.Diagnostics.Stopwatch]::StartNew()
while ($null -eq $sessionItem) {
    $sessionItem = Find-SessionItem $root
    if ($null -ne $sessionItem) {
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
if ($null -eq $sessionItem) {
    $toggle = Find-ToggleSidebar $root
    if ($null -eq $toggle) {
        throw "Could not find a ZCode session titled '$TargetTitle'"
    }
    if (-not (Invoke-Element $toggle)) {
        throw "The ZCode sidebar toggle has no invoke pattern"
    }
    $sidebarOpened = $true
    $sessionItem = Wait-For { Find-SessionItem $root } 4000
    if ($null -eq $sessionItem) {
        # Older sessions are hidden behind a Show more expander.
        $showMore = Find-ShowMore $root
        if ($null -ne $showMore) {
            $null = Invoke-Element $showMore
            $sessionItem = Wait-For { Find-SessionItem $root } 4000
        }
    }
    if ($null -eq $sessionItem) {
        # Undo the sidebar toggle so the user's layout is left unchanged.
        $undo = Find-ToggleSidebar $root
        if ($null -ne $undo) {
            $null = Invoke-Element $undo
        }
        throw "Could not find a ZCode session titled '$TargetTitle'"
    }
}

if (-not (Invoke-Element $sessionItem)) {
    throw "The ZCode session row for '$TargetTitle' has no invoke or selection pattern"
}

# Confirm the app actually activated the session: while it is merely listed,
# the exact title appears once (the sidebar row); once active it also appears
# as the conversation header. Waiting here keeps racing observers (and users)
# from turning a slow switch into a false negative.
$activated = $false
$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.ElapsedMilliseconds -lt 5000) {
    $exact = 0
    $elements = $root.FindAll($descendants, $allElements)
    for ($index = 0; $index -lt $elements.Count; $index++) {
        $element = $elements.Item($index)
        if ($element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Text -and
            $element.Current.Name -eq $TargetTitle) {
            $exact++
        }
    }
    if ($exact -ge 2) {
        $activated = $true
        break
    }
    Start-Sleep -Milliseconds 200
}

if ($sidebarOpened) {
    Start-Sleep -Milliseconds 400
    $toggle = Find-ToggleSidebar $root
    if ($null -ne $toggle) {
        $null = Invoke-Element $toggle
    }
}

if (-not $activated) {
    throw "The ZCode session '$TargetTitle' did not activate after selection"
}

[Console]::Out.WriteLine("selected")
