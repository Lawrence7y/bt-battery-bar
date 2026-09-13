// bt-battery-bar 图形卸载程序（无控制台窗口）
using System.Drawing;
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using Microsoft.Win32;

namespace BtBatteryBarUninstall
{
    static class Program
    {
        [STAThread]
        static int Main(string[] args)
        {
            bool silent = false;
            foreach (string a in args)
            {
                string t = a.Trim();
                if (t.Equals("/S", StringComparison.OrdinalIgnoreCase) ||
                    t.Equals("/silent", StringComparison.OrdinalIgnoreCase) ||
                    t.Equals("-s", StringComparison.OrdinalIgnoreCase))
                    silent = true;
            }

            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            if (silent)
            {
                try { Uninstall(); Console.WriteLine("uninstalled"); return 0; }
                catch (Exception ex) { Console.WriteLine("error: " + ex.Message); return 1; }
            }

            using (ConfirmForm f = new ConfirmForm())
            {
                if (f.ShowDialog() != DialogResult.OK) return 1;
            }
            try { Uninstall(); }
            catch (Exception ex)
            {
                MessageBox.Show("卸载失败：" + ex.Message, "bt-battery-bar 卸载", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 1;
            }
            return 0;
        }

        static void Uninstall()
        {
            // 安装目录 = 本程序所在目录
            string dir = Path.GetDirectoryName(Application.ExecutablePath);

            // 先切换工作目录，避免本进程占用安装目录导致无法删除
            try { Environment.CurrentDirectory = Path.GetTempPath(); } catch { }

            foreach (Process p in Process.GetProcessesByName("bt-battery-bar"))
            {
                try { p.Kill(); } catch { }
            }
            System.Threading.Thread.Sleep(400);

            // 移除开机自启
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run", true))
                {
                    if (key != null) key.DeleteValue("BtBatteryBar", false);
                }
            }
            catch { }

            // 移除卸载信息
            try
            {
                Registry.CurrentUser.DeleteSubKeyTree(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\BtBatteryBar", false);
            }
            catch { }

            // 删除安装目录
            try
            {
                if (dir != null && Directory.Exists(dir))
                {
                    foreach (string f in Directory.GetFiles(dir))
                    {
                        try { File.SetAttributes(f, FileAttributes.Normal); } catch { }
                        try { File.Delete(f); } catch { }
                    }
                    try { Directory.Delete(dir, true); } catch { }
                }
            }
            catch { }

            // 自删除：正在运行的映像无法当场删除，用 MoveFileEx 标记为重启时删除。
            // 不经过 cmd / shell，避免任何命令拼接（目录在重启时随自身一同清除）
            try
            {
                string self = Application.ExecutablePath;
                MoveFileEx(self, null, MOVEFILE_DELAY_UNTIL_REBOOT);
                MoveFileEx(dir, null, MOVEFILE_DELAY_UNTIL_REBOOT);
            }
            catch { }
        }

        const int MOVEFILE_DELAY_UNTIL_REBOOT = 0x4;

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern bool MoveFileEx(string lpExistingFileName, string lpNewFileName, int dwFlags);
    }

    class ConfirmForm : Form
    {
        public ConfirmForm()
        {
            Text = "bt-battery-bar 卸载";
            StartPosition = FormStartPosition.CenterScreen;
            FormBorderStyle = FormBorderStyle.FixedSingle;
            MaximizeBox = false;
            MinimizeBox = false;
            ClientSize = new Size(420, 170);
            Font = new Font("Microsoft YaHei UI", 9F);
            BackColor = Color.White;
            try { Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch { }

            Label lbl = new Label();
            lbl.Text = "确定要卸载 bt-battery-bar 吗？\n\n将停止程序、移除开机自启，并删除安装目录中的文件。";
            lbl.Location = new Point(24, 22);
            lbl.Size = new Size(372, 60);
            Controls.Add(lbl);

            Button btnOk = new Button();
            btnOk.Text = "卸载";
            btnOk.Size = new Size(110, 32);
            btnOk.Location = new Point(176, 110);
            btnOk.DialogResult = DialogResult.OK;
            Controls.Add(btnOk);

            Button btnCancel = new Button();
            btnCancel.Text = "取消";
            btnCancel.Size = new Size(110, 32);
            btnCancel.Location = new Point(294, 110);
            btnCancel.DialogResult = DialogResult.Cancel;
            Controls.Add(btnCancel);

            AcceptButton = btnOk;
            CancelButton = btnCancel;
        }
    }
}
