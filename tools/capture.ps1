# Снимок окна Modly (даже перекрытого другими окнами) в PNG через PrintWindow.
#   powershell -ExecutionPolicy Bypass -File tools\capture.ps1 -Out shot.png [-Width 1300 -Height 900]
param([string]$Title = 'Modly', [string]$Out, [int]$Width = 0, [int]$Height = 0)
Add-Type -ReferencedAssemblies System.Drawing @"
using System; using System.Runtime.InteropServices; using System.Drawing; using System.Drawing.Imaging; using System.Threading;
public static class Cap {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
  public struct RECT { public int L, T, R, B; }
  public static string Shot(string title, string path, int w0, int h0) {
    SetProcessDPIAware();
    IntPtr h = FindWindow(null, title); if (h == IntPtr.Zero) return "nowin";
    if (w0 > 0 && h0 > 0) { SetWindowPos(h, IntPtr.Zero, 0, 0, w0, h0, 0x0004 | 0x0010); Thread.Sleep(900); }
    RECT r; GetWindowRect(h, out r); int w = r.R - r.L, hh = r.B - r.T;
    using (var bmp = new Bitmap(w, hh)) {
      using (var g = Graphics.FromImage(bmp)) { IntPtr dc = g.GetHdc(); PrintWindow(h, dc, 2); g.ReleaseHdc(dc); }
      bmp.Save(path, ImageFormat.Png);
    }
    return w + "x" + hh;
  }
}
"@
[Cap]::Shot($Title, $Out, $Width, $Height)
