$ErrorActionPreference = "Stop"

trap {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

if ($null -eq $WindowHandle) {
    throw "ZCode window handle was not supplied"
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
$controlView = [System.Windows.Automation.TreeWalker]::ControlViewWalker

function Invoke-AccessibilityPoke {
    param([IntPtr]$Window)

    $target = [ZCodeMsaaTap]::FindWindowExW($Window, [IntPtr]::Zero, "Chrome_RenderWidgetHostHWND", $null)
    if ($target -eq [IntPtr]::Zero) {
        $target = $Window
    }
    $iid = [Guid]"618736e0-3c3d-11cf-810c-00aa00389b71"  # IAccessible
    $obj = $null
    $null = [ZCodeMsaaTap]::AccessibleObjectFromWindow($target, [uint32]4294967280, [ref]$iid, [ref]$obj)  # OBJID_CLIENT
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

# The sidebar exposes several "New task" buttons: every project row carries
# one, and the Tasks section has its own. Only the section-level button starts
# a project-neutral task, so it is identified structurally: it is the "New
# task" button whose control-view parent is the group named "Tasks".
function Find-NewTaskButton {
    param([System.Windows.Automation.AutomationElement]$SearchRoot)

    return Find-Element $SearchRoot {
        param($element)
        if ($element.Current.ControlType -ne [System.Windows.Automation.ControlType]::Button) {
            return $false
        }
        if ($element.Current.Name -ne "New task") {
            return $false
        }
        $parent = $controlView.GetParent($element)
        return $null -ne $parent -and $parent.Current.Name -eq "Tasks"
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

$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
if ($null -eq $root) {
    throw "ZCode window is not available to Windows UI Automation"
}

# Keep polling until the button materializes. A ZCode that has been running
# for a while without any automation client tears its accessibility tree
# down, and rebuilding it can take many seconds, so the budget is generous.
# The sidebar toggle bounds the wait: once it is visible the tree is warm,
# so a still-missing button means the sidebar is collapsed rather than the
# tree being cold.
$newTaskButton = $null
$warmup = [System.Diagnostics.Stopwatch]::StartNew()
while ($null -eq $newTaskButton) {
    $newTaskButton = Find-NewTaskButton $root
    if ($null -ne $newTaskButton) {
        break
    }
    if ($warmup.ElapsedMilliseconds -ge 20000) {
        break
    }
    if ($warmup.ElapsedMilliseconds -ge 1500 -and $null -ne (Find-ToggleSidebar $root)) {
        break
    }
    Invoke-AccessibilityPoke ([IntPtr]$WindowHandle)
    Start-Sleep -Milliseconds 250
}

# The Tasks section lives in the collapsible sidebar. When the user keeps it
# collapsed the button is not rendered at all, so open it for the click and
# restore the collapsed state afterwards.
$sidebarOpened = $false
if ($null -eq $newTaskButton) {
    $toggle = Find-ToggleSidebar $root
    if ($null -eq $toggle) {
        throw "Could not find the ZCode new task button"
    }
    if (-not (Invoke-Element $toggle)) {
        throw "The ZCode sidebar toggle has no invoke pattern"
    }
    $sidebarOpened = $true
    $newTaskButton = Wait-For { Find-NewTaskButton $root } 4000
    if ($null -eq $newTaskButton) {
        # Undo the sidebar toggle so the user's layout is left unchanged.
        $undo = Find-ToggleSidebar $root
        if ($null -ne $undo) {
            $null = Invoke-Element $undo
        }
        throw "Could not find the ZCode new task button"
    }
}

if (-not (Invoke-Element $newTaskButton)) {
    throw "The ZCode new task button has no invoke pattern"
}

if ($sidebarOpened) {
    Start-Sleep -Milliseconds 400
    $toggle = Find-ToggleSidebar $root
    if ($null -ne $toggle) {
        $null = Invoke-Element $toggle
    }
}

[Console]::Out.WriteLine("created")
