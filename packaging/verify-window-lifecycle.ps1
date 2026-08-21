# TrontSnap black-box window lifecycle verifier. Run non-elevated in the
# interactive desktop after installing and launching TrontSnap.
#
# This deliberately drives the real custom chrome and notification icon. The
# install verifier cannot catch a window that is painted but still minimized in
# winit, or a restored egui surface that no longer accepts pointer input.
#
# Exit code 0 means every lifecycle and interaction check passed. Exit code 1
# means at least one check failed.

$ErrorActionPreference = 'Stop'
$script:Failures = 0
$script:OriginalCursor = $null
$script:MainHwnd = [IntPtr]::Zero
$script:MainElement = $null
$script:ProcessId = 0
$script:OriginallyTopmost = $false
$script:TopmostGuardApplied = $false

function Check($name, $ok, $detail) {
    if ($ok) {
        Write-Host ("  [ PASS ] {0,-46} {1}" -f $name, $detail)
    } else {
        $script:Failures++
        Write-Host ("  [ FAIL ] {0,-46} {1}" -f $name, $detail)
    }
}

function Wait-Until([scriptblock]$predicate, [int]$timeoutMs = 4000, [int]$pollMs = 50) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
    do {
        try {
            if (& $predicate) { return $true }
        } catch {
            # AccessKit replaces nodes as egui redraws. A stale element during a
            # poll is expected; the next poll reacquires it.
        }
        Start-Sleep -Milliseconds $pollMs
    } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class TrontSnapLifecycleNative
{
    public const int SW_HIDE = 0;
    public const int SW_SHOW = 5;
    public const uint WM_SYSCOMMAND = 0x0112;
    public const int SC_RESTORE = 0xF120;
    public const uint WM_KEYDOWN = 0x0100;
    public const int VK_ESCAPE = 0x1B;
    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_TOPMOST = 0x00000008;
    private const uint SWP_NOSIZE = 0x0001;
    private const uint SWP_NOMOVE = 0x0002;
    private const uint SWP_NOACTIVATE = 0x0010;
    private const uint INPUT_MOUSE = 0;
    private const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    private const uint MOUSEEVENTF_LEFTUP = 0x0004;
    private const uint DWMWA_EXTENDED_FRAME_BOUNDS = 9;
    private static readonly IntPtr DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = new IntPtr(-4);

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X; public int Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    private struct MOUSEINPUT
    {
        public int Dx;
        public int Dy;
        public uint MouseData;
        public uint Flags;
        public uint Time;
        public UIntPtr ExtraInfo;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct INPUTUNION
    {
        [FieldOffset(0)] public MOUSEINPUT Mouse;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct INPUT
    {
        public uint Type;
        public INPUTUNION Union;
    }

    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassNameW(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hwnd, int command);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();
    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")]
    private static extern bool AttachThreadInput(uint attach, uint attachTo, bool enable);
    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint inputCount, INPUT[] inputs, int inputSize);
    [DllImport("user32.dll")]
    private static extern uint GetDoubleClickTime();
    [DllImport("dwmapi.dll")]
    private static extern int DwmGetWindowAttribute(
        IntPtr hwnd,
        uint attribute,
        out RECT value,
        uint valueSize);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetThreadDpiAwarenessContext(IntPtr dpiContext);
    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")]
    private static extern bool PostMessageW(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    private static extern IntPtr SendMessageW(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowLongW(IntPtr hwnd, int index);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool SetWindowPos(
        IntPtr hwnd,
        IntPtr insertAfter,
        int x,
        int y,
        int width,
        int height,
        uint flags);

    public static IntPtr FindMainWindow(int wantedProcessId)
    {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr hwnd, IntPtr unused) {
            uint processId;
            GetWindowThreadProcessId(hwnd, out processId);
            if (processId != (uint)wantedProcessId) return true;
            var title = new StringBuilder(256);
            GetWindowTextW(hwnd, title, title.Capacity);
            if (title.ToString() == "TrontSnap") {
                result = hwnd;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static POINT CaptionButtonPoint(IntPtr hwnd, bool closeButton)
    {
        RECT bounds;
        int result = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            out bounds,
            (uint)Marshal.SizeOf(typeof(RECT)));
        if (result != 0 || bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top) {
            // GetWindowRect is DPI-virtualized for an unaware caller. Temporarily
            // make this thread per-monitor aware so the fallback is also physical.
            IntPtr previousContext = SetThreadDpiAwarenessContext(
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if (previousContext == IntPtr.Zero) {
                throw new InvalidOperationException(
                    "DwmGetWindowAttribute failed and physical GetWindowRect fallback is unavailable");
            }
            try {
                if (!GetWindowRect(hwnd, out bounds) ||
                    bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top) {
                    throw new InvalidOperationException(
                        "DwmGetWindowAttribute and GetWindowRect failed");
                }
            } finally {
                SetThreadDpiAwarenessContext(previousContext);
            }
        }
        uint dpi = GetDpiForWindow(hwnd);
        if (dpi == 0) dpi = 96;
        double scale = dpi / 96.0;
        // app.rs caption_overlay: 4 px right margin, 3 x 40 px buttons,
        // 5 px top margin, and 30 px button height.
        double fromRight = closeButton ? 24.0 : 104.0;
        return new POINT {
            X = bounds.Right - (int)Math.Round(fromRight * scale),
            Y = bounds.Top + (int)Math.Round(20.0 * scale)
        };
    }

    public static void MoveTo(int x, int y)
    {
        if (!SetCursorPos(x, y)) throw new InvalidOperationException("SetCursorPos failed");
    }

    public static bool Activate(IntPtr hwnd)
    {
        RequestForeground(hwnd);
        Thread.Sleep(200);
        return GetForegroundWindow() == hwnd;
    }

    private static void RequestForeground(IntPtr hwnd)
    {
        IntPtr foreground = GetForegroundWindow();
        uint ignored;
        uint foregroundThread = GetWindowThreadProcessId(foreground, out ignored);
        uint currentThread = GetCurrentThreadId();
        bool attached = foregroundThread != 0 && foregroundThread != currentThread &&
            AttachThreadInput(currentThread, foregroundThread, true);
        try {
            SetForegroundWindow(hwnd);
        } finally {
            if (attached) AttachThreadInput(currentThread, foregroundThread, false);
        }
    }

    public static bool IsForeground(IntPtr hwnd)
    {
        return GetForegroundWindow() == hwnd;
    }

    public static bool IsTopmost(IntPtr hwnd)
    {
        return (GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST) != 0;
    }

    public static bool SetTopmostGuard(IntPtr hwnd, bool topmost)
    {
        IntPtr band = topmost ? new IntPtr(-1) : new IntPtr(-2); // HWND_TOPMOST / HWND_NOTOPMOST
        bool ok = SetWindowPos(
            hwnd,
            band,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        return ok && IsTopmost(hwnd) == topmost;
    }

    public static void RestoreForCleanup(IntPtr hwnd)
    {
        if (IsIconic(hwnd)) {
            SendMessageW(hwnd, WM_SYSCOMMAND, new IntPtr(SC_RESTORE), IntPtr.Zero);
        }
        ShowWindow(hwnd, SW_SHOW);
        if (IsIconic(hwnd)) {
            SendMessageW(hwnd, WM_SYSCOMMAND, new IntPtr(SC_RESTORE), IntPtr.Zero);
        }
    }

    public static void ClickAt(int x, int y)
    {
        MoveTo(x, y);
        Thread.Sleep(80);
        SendLeftClick();
    }

    public static bool ActivateAndClickAt(IntPtr hwnd, int x, int y)
    {
        MoveTo(x, y);
        Thread.Sleep(80);
        for (int attempt = 0; attempt < 6; attempt++) {
            RequestForeground(hwnd);
            Thread.Sleep(75);
            if (GetForegroundWindow() == hwnd) {
                // Require foreground to remain stable for a second sample. Only
                // then send the one measured target click, with no caller-side
                // output or polling window between activation and SendInput.
                Thread.Sleep(40);
                if (GetForegroundWindow() == hwnd) {
                    SendLeftClick();
                    return true;
                }
            }
            Thread.Sleep(50);
        }
        return false;
    }

    public static void DoubleClickAt(int x, int y)
    {
        MoveTo(x, y);
        // Let Explorer finish its hover/target transition before injecting the
        // first press. Both clicks remain comfortably inside the application's
        // 330 ms single-click arbitration window and Windows' own setting.
        Thread.Sleep(120);
        SendLeftClick();
        uint configured = GetDoubleClickTime();
        int gap = (int)Math.Max(60, Math.Min(120, configured / 3));
        Thread.Sleep(gap);
        SendLeftClick();
    }

    private static void SendLeftClick()
    {
        var inputs = new INPUT[] {
            new INPUT {
                Type = INPUT_MOUSE,
                Union = new INPUTUNION {
                    Mouse = new MOUSEINPUT { Flags = MOUSEEVENTF_LEFTDOWN }
                }
            },
            new INPUT {
                Type = INPUT_MOUSE,
                Union = new INPUTUNION {
                    Mouse = new MOUSEINPUT { Flags = MOUSEEVENTF_LEFTUP }
                }
            }
        };
        uint sent = SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
        if (sent != (uint)inputs.Length) {
            throw new InvalidOperationException(
                "SendInput sent " + sent + " of " + inputs.Length +
                " mouse events; Win32 error " + Marshal.GetLastWin32Error());
        }
    }

    public static void CancelRegionOverlays(int wantedProcessId)
    {
        var overlays = new List<IntPtr>();
        EnumWindows(delegate(IntPtr hwnd, IntPtr unused) {
            uint processId;
            GetWindowThreadProcessId(hwnd, out processId);
            if (processId != (uint)wantedProcessId) return true;
            var className = new StringBuilder(256);
            GetClassNameW(hwnd, className, className.Capacity);
            if (className.ToString() == "TrontSnapRegionOverlay") overlays.Add(hwnd);
            return true;
        }, IntPtr.Zero);
        foreach (IntPtr hwnd in overlays) {
            PostMessageW(hwnd, WM_KEYDOWN, new IntPtr(VK_ESCAPE), IntPtr.Zero);
        }
    }
}
'@

function Get-AppElements {
    return $script:MainElement.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition
    )
}

function Find-TabElement([string]$name) {
    foreach ($element in (Get-AppElements)) {
        try {
            if ($element.Current.Name -ne $name -or $element.Current.IsOffscreen) { continue }
            $pattern = $null
            if ($element.TryGetCurrentPattern(
                    [System.Windows.Automation.TogglePattern]::Pattern,
                    [ref]$pattern)) {
                return $element
            }
        } catch { }
    }
    return $null
}

function Test-TabOn([string]$name) {
    $element = Find-TabElement $name
    if ($null -eq $element) { return $false }
    try {
        $pattern = $null
        if (-not $element.TryGetCurrentPattern(
                [System.Windows.Automation.TogglePattern]::Pattern,
                [ref]$pattern)) { return $false }
        return $pattern.Current.ToggleState -eq [System.Windows.Automation.ToggleState]::On
    } catch {
        return $false
    }
}

function Test-UiaHit([int]$x, [int]$y, [string]$name) {
    try {
        $point = New-Object System.Windows.Point($x, $y)
        $hit = [System.Windows.Automation.AutomationElement]::FromPoint($point)
        $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
        for ($depth = 0; $depth -lt 6 -and $null -ne $hit; $depth++) {
            if ($hit.Current.ProcessId -eq $script:ProcessId -and $hit.Current.Name -eq $name) {
                return $true
            }
            $hit = $walker.GetParent($hit)
        }
    } catch { }
    return $false
}

function Get-UiaHitDescription([int]$x, [int]$y) {
    try {
        $point = New-Object System.Windows.Point($x, $y)
        $hit = [System.Windows.Automation.AutomationElement]::FromPoint($point)
        $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
        $parts = @()
        for ($depth = 0; $depth -lt 5 -and $null -ne $hit; $depth++) {
            $current = $hit.Current
            $parts += "depth=$depth pid=$($current.ProcessId) name='$($current.Name)' type=$($current.ControlType.ProgrammaticName)"
            $hit = $walker.GetParent($hit)
        }
        return $parts -join '; '
    } catch {
        return "AutomationElement.FromPoint failed: $($_.Exception.Message)"
    }
}

function Get-ElementCenter([System.Windows.Automation.AutomationElement]$element) {
    $rect = $element.Current.BoundingRectangle
    return [pscustomobject]@{
        X = [int][Math]::Round($rect.Left + ($rect.Width / 2.0))
        Y = [int][Math]::Round($rect.Top + ($rect.Height / 2.0))
    }
}

function Try-ActivateMainWindow([bool]$allowPhysicalPrimer = $true) {
    $attempts = 0
    for ($i = 0; $i -lt 3; $i++) {
        $attempts++
        if ([TrontSnapLifecycleNative]::Activate($script:MainHwnd)) {
            return [pscustomobject]@{ Ok = $true; Attempts = $attempts; Primer = 'none' }
        }
        Start-Sleep -Milliseconds 100
    }

    $primerKind = 'uia-focus'
    if ($allowPhysicalPrimer) {
        # Setup for caption-button tests may use a harmless click on the
        # already-selected Gallery tab when Windows denies SetForegroundWindow.
        $primer = Find-TabElement 'Gallery'
        try {
            if ($null -ne $primer) {
                $point = Get-ElementCenter $primer
                [TrontSnapLifecycleNative]::ClickAt($point.X, $point.Y)
                $primerKind = 'gallery-click'
                Start-Sleep -Milliseconds 200
            } else {
                $script:MainElement.SetFocus()
            }
        } catch { }
    } else {
        # Tab verification is strict: do not consume a physical chrome click as
        # an activation primer before the one target click being measured.
        try { $script:MainElement.SetFocus() } catch { }
    }

    for ($i = 0; $i -lt 3; $i++) {
        $attempts++
        if ([TrontSnapLifecycleNative]::Activate($script:MainHwnd)) {
            return [pscustomobject]@{ Ok = $true; Attempts = $attempts; Primer = $primerKind }
        }
        Start-Sleep -Milliseconds 100
    }
    return [pscustomobject]@{ Ok = $false; Attempts = $attempts; Primer = $primerKind }
}

function Ensure-MainForeground([string]$checkName, [bool]$allowPhysicalPrimer = $true) {
    $result = Try-ActivateMainWindow $allowPhysicalPrimer
    $detail = if ($result.Ok) {
        "foreground HWND matches after $($result.Attempts) attempt(s); primer=$($result.Primer)"
    } else {
        "foreground HWND never matched after $($result.Attempts) attempt(s); primer=$($result.Primer)"
    }
    Check $checkName $result.Ok $detail
    return $result.Ok
}

function Wait-LiveTabTarget([string]$name, [int]$timeoutMs = 5000) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
    $exposed = $false
    $stableHits = 0
    $polls = 0
    $lastPoint = $null
    $diagnostic = 'no UIA element was available'
    do {
        $polls++
        # The terminal hosting this verifier may reclaim foreground while it
        # processes output. Quietly reactivate on every stabilization poll so
        # AutomationElement.FromPoint measures the app rather than that terminal.
        [void][TrontSnapLifecycleNative]::Activate($script:MainHwnd)
        $element = Find-TabElement $name
        if ($null -eq $element) {
            $stableHits = 0
            Start-Sleep -Milliseconds 75
            continue
        }
        $exposed = $true

        $point = Get-ElementCenter $element
        [TrontSnapLifecycleNative]::MoveTo($point.X, $point.Y)
        Start-Sleep -Milliseconds 75
        if (Test-UiaHit $point.X $point.Y $name) {
            if ($null -ne $lastPoint -and
                [Math]::Abs($lastPoint.X - $point.X) -le 2 -and
                [Math]::Abs($lastPoint.Y - $point.Y) -le 2) {
                $stableHits++
            } else {
                $stableHits = 1
            }
            $lastPoint = $point
            if ($stableHits -ge 2) {
                return [pscustomobject]@{
                    Ok = $true
                    Exposed = $true
                    Point = $point
                    Polls = $polls
                    Diagnostic = 'two consecutive live UIA hit samples'
                }
            }
        } else {
            $stableHits = 0
            $lastPoint = $point
            $diagnostic = Get-UiaHitDescription $point.X $point.Y
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)

    return [pscustomobject]@{
        Ok = $false
        Exposed = $exposed
        Point = $lastPoint
        Polls = $polls
        Diagnostic = $diagnostic
    }
}

function Click-Tab([string]$name) {
    # No physical primer and no Check/Write-Host before the target click. The
    # only physical chrome click in this function is the one Off -> On action.
    $activation = Try-ActivateMainWindow $false
    $target = Wait-LiveTabTarget $name 5000
    $pointDetail = if ($null -ne $target.Point) {
        "point=$($target.Point.X),$($target.Point.Y)"
    } else {
        'no point'
    }
    $wasSelected = $false
    $clickSent = $false
    $isSelected = $false
    if ($target.Ok) {
        $wasSelected = Test-TabOn $name
        $clickSent = [TrontSnapLifecycleNative]::ActivateAndClickAt(
            $script:MainHwnd,
            $target.Point.X,
            $target.Point.Y
        )
        if ($clickSent) {
            $isSelected = Wait-Until { Test-TabOn $name } 3000 50
        }
    }

    $transitioned = (-not $wasSelected) -and $isSelected
    # All reporting is intentionally after the atomic activation/click above.
    Check "$name tab window foregrounded" $clickSent $(if ($clickSent) { "atomic foreground stable at click; setup attempts=$($activation.Attempts)" } else { "no click sent; setup foreground=$($activation.Ok) liveTarget=$($target.Ok)" })
    Check "$name tab exposed after restore" $target.Exposed $(if ($target.Exposed) { "UIA TogglePattern element found within $($target.Polls) poll(s)" } else { "UIA element missing after $($target.Polls) poll(s)" })
    Check "$name tab hover hit-test" $target.Ok $(if ($target.Ok) { "$pointDetail; two consecutive samples" } else { "$pointDetail; actual hit: $($target.Diagnostic)" })
    Check "$name tab accepts one physical click" $transitioned $(if ($transitioned) { 'ToggleState Off -> On after exactly one target click' } else { "before=$wasSelected after=$isSelected; no retry click sent" })
    return $transitioned
}

function Find-TrayIcon {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
        'NotifyItemIcon'
    )
    foreach ($element in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)) {
        try {
            if (-not $element.Current.IsOffscreen -and $element.Current.Name -like 'TrontSnap:*') {
                return $element
            }
        } catch { }
    }
    return $null
}

function Get-TrayClickPoint {
    $target = Find-TrayIcon
    if ($null -eq $target) { return $null }
    $rect = $target.Current.BoundingRectangle

    # Windows 11 sometimes reports the target's Width as the rest of the whole
    # notification area. The next visible notification item still starts at the
    # correct x coordinate, so use that boundary and fall back to a 32 px slot.
    $slotWidth = $rect.Width
    if ($slotWidth -lt 16 -or $slotWidth -gt 64) {
        $nextX = [double]::PositiveInfinity
        $root = [System.Windows.Automation.AutomationElement]::RootElement
        $condition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
            'NotifyItemIcon'
        )
        foreach ($element in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)) {
            try {
                $other = $element.Current.BoundingRectangle
                if (-not $element.Current.IsOffscreen -and
                    [Math]::Abs($other.Top - $rect.Top) -lt 2 -and
                    $other.Left -gt $rect.Left -and $other.Left -lt $nextX) {
                    $nextX = $other.Left
                }
            } catch { }
        }
        if (-not [double]::IsPositiveInfinity($nextX)) {
            $slotWidth = $nextX - $rect.Left
        } else {
            $slotWidth = 32
        }
    }
    return [pscustomobject]@{
        X = [int][Math]::Round($rect.Left + ($slotWidth / 2.0))
        Y = [int][Math]::Round($rect.Top + ($rect.Height / 2.0))
        Name = $target.Current.Name
    }
}

function Invoke-TrayDoubleClick {
    $point = Get-TrayClickPoint
    if ($null -eq $point) {
        Check 'real TrontSnap tray icon discovered' $false 'visible NotifyItemIcon not found'
        return $false
    }
    Check 'real TrontSnap tray icon discovered' $true "$($point.Name) at $($point.X),$($point.Y)"
    # tray-icon 0.19.3 receives the Shell notification sequence
    # WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP and maps the
    # third message to TrayIconEvent::DoubleClick. SendInput produces that real
    # Explorer sequence; directly posting app messages would not test the icon.
    [TrontSnapLifecycleNative]::DoubleClickAt($point.X, $point.Y)
    return $true
}

function Test-VisibleRestored {
    return [TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd) -and
        -not [TrontSnapLifecycleNative]::IsIconic($script:MainHwnd)
}

"TrontSnap window lifecycle verification  ($(Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))"
""

try {
    $point = New-Object TrontSnapLifecycleNative+POINT
    if ([TrontSnapLifecycleNative]::GetCursorPos([ref]$point)) {
        $script:OriginalCursor = $point
    }

    $installExe = Join-Path $env:ProgramFiles 'TrontSnap\trontsnap.exe'
    if (-not (Test-Path $installExe)) {
        Check 'installed UI exists' $false $installExe
        throw 'installed TrontSnap was not found'
    }
    Check 'installed UI exists' $true $installExe

    $processes = @(Get-Process trontsnap -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -ieq $installExe } catch { $false }
    })
    if ($processes.Count -ne 1) {
        Check 'one installed UI process is running' $false "found $($processes.Count)"
        throw 'launch the installed TrontSnap once before running this verifier'
    }
    $script:ProcessId = $processes[0].Id
    Check 'one installed UI process is running' $true "pid=$($script:ProcessId)"

    $script:MainHwnd = [TrontSnapLifecycleNative]::FindMainWindow($script:ProcessId)
    if ($script:MainHwnd -eq [IntPtr]::Zero) {
        Check 'main HWND discovered' $false 'top-level TrontSnap window not found'
        throw 'main HWND unavailable'
    }
    Check 'main HWND discovered' $true ('hwnd=0x{0:X}' -f $script:MainHwnd.ToInt64())

    # Harness isolation only. The maximized terminal driving this script can
    # otherwise reclaim and cover the app between UIA polling and SendInput.
    # Enter the topmost band without activation, then restore the exact original
    # band in finally. Minimize, hide, restore, and click paths remain real.
    $script:OriginallyTopmost = [TrontSnapLifecycleNative]::IsTopmost($script:MainHwnd)
    if ($script:OriginallyTopmost) {
        $script:TopmostGuardApplied = $true
    } else {
        $script:TopmostGuardApplied = [TrontSnapLifecycleNative]::SetTopmostGuard(
            $script:MainHwnd,
            $true
        )
    }
    Check 'temporary topmost harness guard' $script:TopmostGuardApplied $(if ($script:TopmostGuardApplied) { "active; originallyTopmost=$($script:OriginallyTopmost)" } else { 'SetWindowPos(HWND_TOPMOST) failed' })
    if (-not $script:TopmostGuardApplied) { throw 'could not isolate app HWND from the harness terminal' }

    $script:MainElement = [System.Windows.Automation.AutomationElement]::FromHandle($script:MainHwnd)
    if ($null -eq $script:MainElement) {
        Check 'main AccessKit window discovered' $false 'AutomationElement.FromHandle returned null'
        throw 'main AccessKit window unavailable'
    }
    Check 'main AccessKit window discovered' $true $script:MainElement.Current.Name

    $ready = Test-VisibleRestored
    Check 'precondition: gallery starts restored' $ready $(if ($ready) { 'Visible=True Iconic=False' } else { 'open the gallery before running this verifier' })
    if (-not $ready) { throw 'gallery must start visible and restored' }

    $tabsReady = Wait-Until { $null -ne (Find-TabElement 'Gallery') } 4000 50
    Check 'AccessKit chrome tree is live' $tabsReady $(if ($tabsReady) { 'Gallery tab exposed' } else { 'Gallery tab missing' })
    if (-not $tabsReady) { throw 'visible gallery has no live AccessKit chrome tree' }

    "WINDOW MINIMIZE -> REAL TRAY DOUBLE-CLICK"
    $minPoint = [TrontSnapLifecycleNative]::CaptionButtonPoint($script:MainHwnd, $false)
    $foreground = [TrontSnapLifecycleNative]::ActivateAndClickAt(
        $script:MainHwnd,
        $minPoint.X,
        $minPoint.Y
    )
    if ($foreground) {
        $minimized = Wait-Until {
            [TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd) -and
            [TrontSnapLifecycleNative]::IsIconic($script:MainHwnd)
        } 10000 50
    } else {
        $minimized = $false
    }
    Check 'window foregrounded before minimize' $foreground $(if ($foreground) { 'atomic foreground stable at click' } else { 'no click sent because foreground never stabilized' })
    Check 'custom minimize button minimizes window' $minimized $(if ($minimized) { 'Visible=True Iconic=True' } else { 'window never became iconic' })

    if ($minimized) {
        if (Invoke-TrayDoubleClick) {
            $restored = Wait-Until { Test-VisibleRestored } 4000 50
            Check 'tray double-click restores minimized window' $restored $(if ($restored) { 'Visible=True Iconic=False' } else { "Visible=$([TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd)) Iconic=$([TrontSnapLifecycleNative]::IsIconic($script:MainHwnd))" })
        } else {
            $restored = $false
            Check 'tray double-click restores minimized window' $false 'not attempted because tray icon was unavailable'
        }
    } else {
        $restored = $false
        Check 'tray double-click restores minimized window' $false 'not attempted because minimize precondition failed'
    }

    if ($restored) {
        [void](Click-Tab 'Settings')
        [void](Click-Tab 'About')
        [void](Click-Tab 'Gallery')
    } else {
        Check 'Settings/About/Gallery interaction after restore' $false 'restore failed; interaction checks skipped'
    }

    "CLOSE TO TRAY -> REAL TRAY DOUBLE-CLICK"
    if (Test-VisibleRestored) {
        $closePoint = [TrontSnapLifecycleNative]::CaptionButtonPoint($script:MainHwnd, $true)
        $foreground = [TrontSnapLifecycleNative]::ActivateAndClickAt(
            $script:MainHwnd,
            $closePoint.X,
            $closePoint.Y
        )
        if ($foreground) {
            $hidden = Wait-Until {
                -not [TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd) -and
                -not [TrontSnapLifecycleNative]::IsIconic($script:MainHwnd)
            } 10000 50
        } else {
            $hidden = $false
        }
        Check 'window foregrounded before close' $foreground $(if ($foreground) { 'atomic foreground stable at click' } else { 'no click sent because foreground never stabilized' })
        Check 'custom close button hides to tray' $hidden $(if ($hidden) { 'Visible=False Iconic=False' } else { "Visible=$([TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd)) Iconic=$([TrontSnapLifecycleNative]::IsIconic($script:MainHwnd))" })

        if ($hidden) {
            if (Invoke-TrayDoubleClick) {
                $reopened = Wait-Until { Test-VisibleRestored } 4000 50
                Check 'tray double-click restores tray-hidden window' $reopened $(if ($reopened) { 'Visible=True Iconic=False' } else { "Visible=$([TrontSnapLifecycleNative]::IsWindowVisible($script:MainHwnd)) Iconic=$([TrontSnapLifecycleNative]::IsIconic($script:MainHwnd))" })
                if ($reopened) {
                    [void](Click-Tab 'Settings')
                    [void](Click-Tab 'Gallery')
                }
            } else {
                Check 'tray double-click restores tray-hidden window' $false 'not attempted because tray icon was unavailable'
            }
        } else {
            Check 'tray double-click restores tray-hidden window' $false 'not attempted because tray-hidden precondition failed'
        }
    } else {
        Check 'close-to-tray lifecycle' $false 'window was not restored; stage skipped'
    }
} catch {
    Check 'verifier completed without exception' $false $_.Exception.Message
} finally {
    if ($script:ProcessId -ne 0) {
        [TrontSnapLifecycleNative]::CancelRegionOverlays($script:ProcessId)
    }
    if ($script:MainHwnd -ne [IntPtr]::Zero -and -not (Test-VisibleRestored)) {
        # Cleanup only. Test assertions above always observe app-driven behavior
        # before this restore is attempted.
        [TrontSnapLifecycleNative]::RestoreForCleanup($script:MainHwnd)
    }
    if ($script:MainHwnd -ne [IntPtr]::Zero -and
        $script:TopmostGuardApplied -and
        -not $script:OriginallyTopmost) {
        [void][TrontSnapLifecycleNative]::SetTopmostGuard($script:MainHwnd, $false)
    }
    if ($null -ne $script:OriginalCursor) {
        [void][TrontSnapLifecycleNative]::SetCursorPos($script:OriginalCursor.X, $script:OriginalCursor.Y)
    }
}

""
if ($script:Failures -eq 0) {
    'RESULT: PASS  all window lifecycle checks passed'
    exit 0
}

"RESULT: $($script:Failures) FAILURE(S)"
exit 1
