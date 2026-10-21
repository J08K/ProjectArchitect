# Screenshot of the pa window (for checking UI changes without clicking).
# Usage: tools/shot.ps1 -exe target\debug\pa.exe -data <scratch data dir> -out shot.png [-wait 10] [-extra @("--open","Name","--section","roadmap")]
# The data dir should be a COPY of %APPDATA%\ProjectArchitect (or a seeded one), never the real one. Stops nothing: kill pa afterwards.
param([string]$exe, [string]$data, [string]$out, [int]$wait = 10, [string[]]$extra = @())
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class W { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
public struct R { public int L, T, Ri, B; } }
"@
[W]::SetProcessDPIAware() | Out-Null
$p = Start-Process -FilePath $exe -ArgumentList (@("--data-dir", $data) + $extra) -PassThru -RedirectStandardError "$out.err.txt"
Start-Sleep -Seconds $wait
$p.Refresh()
$h = $p.MainWindowHandle
$r = New-Object W+R; [W]::GetWindowRect($h, [ref]$r) | Out-Null
$bmp = New-Object System.Drawing.Bitmap ($r.Ri - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
[W]::PrintWindow($h, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc); $bmp.Save($out); $g.Dispose()
"saved $out ($($r.Ri - $r.L)x$($r.B - $r.T)) pid $($p.Id)"
