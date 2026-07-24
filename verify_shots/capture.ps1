Add-Type -AssemblyName System.Drawing
$src = @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Cap {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll")] public static extern int PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

  public static IntPtr FindTopWindowForPid(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((hWnd, lParam) => {
      uint wpid;
      GetWindowThreadProcessId(hWnd, out wpid);
      if (wpid == pid && IsWindowVisible(hWnd)) {
        var sb = new StringBuilder(256);
        GetWindowText(hWnd, sb, 256);
        if (sb.Length > 0) {
          found = hWnd;
          return false;
        }
      }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
'@
Add-Type -TypeDefinition $src

$proc = Get-Process trontsnap -ErrorAction SilentlyContinue
if (-not $proc) {
  Write-Output "PROCESS NOT FOUND"
  exit 1
}
$pid_ = $proc.Id
Write-Output "PID: $pid_"

$hwnd = [Win32Cap]::FindTopWindowForPid([uint32]$pid_)
if ($hwnd -eq [IntPtr]::Zero) {
  Write-Output "WINDOW NOT FOUND"
  exit 1
}
Write-Output "HWND: $hwnd"

$rect = New-Object Win32Cap+RECT
[Win32Cap]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
Write-Output "WindowRect size: ${w}x${h}"

# PrintWindow renders the window's own surface directly (no focus/z-order
# change, works even if occluded) -- does not touch the live desktop at all.
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$PW_RENDERFULLCONTENT = 2
$ok = [Win32Cap]::PrintWindow($hwnd, $hdc, $PW_RENDERFULLCONTENT)
$g.ReleaseHdc($hdc)
Write-Output "PrintWindow result: $ok"
$g.Dispose()

$outPath = "C:\trontstack\trontsnap\verify_shots\trontsnap-gradient-01.png"
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "Saved: $outPath"
