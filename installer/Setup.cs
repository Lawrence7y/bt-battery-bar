// bt-battery-bar 图形安装器（无需管理员权限，安装到当前用户目录）
// 使用 .NET Framework WinForms 编译，无控制台窗口。
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Windows.Forms;
using Microsoft.Win32;

namespace BtBatteryBarSetup
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
                    t.Equals("-s", StringComparison.OrdinalIgnoreCase) ||
                    t.Equals("--silent", StringComparison.OrdinalIgnoreCase))
                    silent = true;
            }

            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            if (silent)
            {
                try { SilentInstall(); Console.WriteLine("installed"); return 0; }
                catch (Exception ex) { Console.WriteLine("error: " + ex.Message); return 1; }
            }

            Application.Run(new SetupForm());
            return 0;
        }

        static void SilentInstall()
        {
            InstallerCore core = new InstallerCore(
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "BtBatteryBar"),
                true, true, null);
            core.Install();
        }
    }

    class InstallerCore
    {
        public string DestDir;
        public bool AutoStart;
        public bool LaunchAfter;
        public Action<int, string> Progress;

        public InstallerCore(string destDir, bool autoStart, bool launchAfter, Action<int, string> progress)
        {
            DestDir = destDir;
            AutoStart = autoStart;
            LaunchAfter = launchAfter;
            Progress = progress;
        }

        void Report(int p, string msg)
        {
            if (Progress != null) Progress(p, msg);
        }

        static byte[] ReadResource(string name)
        {
            Assembly asm = Assembly.GetExecutingAssembly();
            string[] names = asm.GetManifestResourceNames();
            foreach (string n in names)
            {
                bool match = string.Equals(n, name, StringComparison.OrdinalIgnoreCase) ||
                             n.IndexOf(name, StringComparison.OrdinalIgnoreCase) >= 0;
                if (match)
                    using (Stream s = asm.GetManifestResourceStream(n))
                    using (MemoryStream ms = new MemoryStream())
                    {
                        s.CopyTo(ms);
                        return ms.ToArray();
                    }
            }
            throw new FileNotFoundException("嵌入资源缺失: " + name);
        }

        public void Install()
        {
            Report(2, "正在停止正在运行的实例…");
            KillApp();

            Report(8, "正在创建安装目录…");
            Directory.CreateDirectory(DestDir);

            Report(15, "正在写入卸载程序…");
            WriteFile(Path.Combine(DestDir, "uninstall.exe"), ReadResource("uninstall.exe"));

            Report(30, "正在复制主程序…");
            WriteFile(Path.Combine(DestDir, "bt-battery-bar.exe"), ReadResource("bt-battery-bar.exe"));

            Report(55, "正在配置开机自启…");
            SetAutoStart(AutoStart);

            Report(70, "正在注册卸载信息…");
            SetUninstallKey();

            Report(88, "完成");

            if (LaunchAfter)
            {
                Report(92, "正在启动 bt-battery-bar…");
                try
                {
                    // 仅启动安装器自行释放、文件名为固定白名单的主程序
                    string mainExe = Path.Combine(DestDir, "bt-battery-bar.exe");
                    if (string.Equals(Path.GetFileName(mainExe), "bt-battery-bar.exe", StringComparison.OrdinalIgnoreCase)
                        && File.Exists(mainExe))
                    {
                        Process.Start(new ProcessStartInfo
                        {
                            FileName = mainExe,
                            UseShellExecute = false
                        });
                    }
                }
                catch { }
            }

            Report(100, "安装完成");
        }

        static void KillApp()
        {
            foreach (System.Diagnostics.Process p in System.Diagnostics.Process.GetProcessesByName("bt-battery-bar"))
            {
                try { p.Kill(); } catch { }
            }
            System.Threading.Thread.Sleep(400);
        }

        static void WriteFile(string path, byte[] data)
        {
            try
            {
                File.Delete(path);
            }
            catch { }
            for (int i = 0; i < 5; i++)
            {
                try
                {
                    using (FileStream fs = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.None))
                    {
                        fs.Write(data, 0, data.Length);
                    }
                    return;
                }
                catch (IOException) { System.Threading.Thread.Sleep(300); }
                catch (UnauthorizedAccessException) { System.Threading.Thread.Sleep(300); }
            }
            throw new IOException("无法写入文件（可能被占用）: " + path);
        }

        void SetAutoStart(bool enable)
        {
            using (RegistryKey key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run", true))
            {
                if (key == null) return;
                if (enable)
                    key.SetValue("BtBatteryBar", "\"" + Path.Combine(DestDir, "bt-battery-bar.exe") + "\"");
                else
                    key.DeleteValue("BtBatteryBar", false);
            }
        }

        void SetUninstallKey()
        {
            string un = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\BtBatteryBar";
            using (RegistryKey key = Registry.CurrentUser.CreateSubKey(un))
            {
                key.SetValue("DisplayName", "bt-battery-bar（任务栏蓝牙/2.4G 电量条）");
                key.SetValue("DisplayVersion", "0.1.3");
                key.SetValue("Publisher", "bt-battery-bar");
                key.SetValue("InstallLocation", DestDir);
                key.SetValue("DisplayIcon", Path.Combine(DestDir, "bt-battery-bar.exe"));
                key.SetValue("UninstallString", "\"" + Path.Combine(DestDir, "uninstall.exe") + "\"");
                key.SetValue("QuietUninstallString", "\"" + Path.Combine(DestDir, "uninstall.exe") + "\" /S");
                key.SetValue("NoModify", 1);
                key.SetValue("NoRepair", 1);
                key.SetValue("EstimatedSize", 1024);
            }
        }
    }

    class SetupForm : Form
    {
        TextBox txtPath;
        CheckBox chkAuto;
        CheckBox chkLaunch;
        Button btnInstall;
        Button btnExit;
        ProgressBar progress;
        Label lblStatus;
        Label lblHeader;
        bool done = false;

        public SetupForm()
        {
            Text = "bt-battery-bar 安装";
            StartPosition = FormStartPosition.CenterScreen;
            FormBorderStyle = FormBorderStyle.FixedSingle;
            MaximizeBox = false;
            MinimizeBox = false;
            ClientSize = new Size(520, 470);
            Font = new Font("Microsoft YaHei UI", 9F);
            BackColor = Color.White;

            try { Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch { }

            // 头部
            Panel header = new Panel();
            header.Dock = DockStyle.Top;
            header.Height = 110;
            header.BackColor = Color.FromArgb(40, 53, 70);
            Controls.Add(header);

            PictureBox icon = new PictureBox();
            icon.Image = DrawBatteryIcon();
            icon.SizeMode = PictureBoxSizeMode.Zoom;
            icon.Location = new Point(18, 18);
            icon.Size = new Size(74, 74);
            header.Controls.Add(icon);

            lblHeader = new Label();
            lblHeader.Text = "bt-battery-bar";
            lblHeader.Font = new Font("Microsoft YaHei UI", 16F, FontStyle.Bold);
            lblHeader.ForeColor = Color.White;
            lblHeader.Location = new Point(104, 24);
            lblHeader.AutoSize = true;
            header.Controls.Add(lblHeader);

            Label lblSub = new Label();
            lblSub.Text = "任务栏内嵌蓝牙 / 2.4G 设备电量条 + 托盘";
            lblSub.Font = new Font("Microsoft YaHei UI", 9.5F);
            lblSub.ForeColor = Color.FromArgb(190, 200, 215);
            lblSub.Location = new Point(104, 58);
            lblSub.AutoSize = true;
            header.Controls.Add(lblSub);

            int y = 130;
            Label lblInfo = new Label();
            lblInfo.Text = "将安装到当前用户目录，无需管理员权限，不弹出命令行窗口。\n安装后可随时从“设置 → 应用”或卸载程序移除。";
            lblInfo.Location = new Point(24, y);
            lblInfo.Size = new Size(472, 44);
            Controls.Add(lblInfo);

            y += 56;
            Label lblPath = new Label();
            lblPath.Text = "安装位置：";
            lblPath.Location = new Point(26, y + 6);
            lblPath.AutoSize = true;
            Controls.Add(lblPath);

            txtPath = new TextBox();
            txtPath.Text = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "BtBatteryBar");
            txtPath.Location = new Point(108, y);
            txtPath.Size = new Size(330, 23);
            Controls.Add(txtPath);

            Button btnBrowse = new Button();
            btnBrowse.Text = "浏览…";
            btnBrowse.Location = new Point(444, y);
            btnBrowse.Size = new Size(56, 25);
            btnBrowse.Click += (s, e) => Browse();
            Controls.Add(btnBrowse);

            y += 40;
            chkAuto = new CheckBox();
            chkAuto.Text = "开机自动启动（写入 HKCU Run）";
            chkAuto.Checked = true;
            chkAuto.Location = new Point(26, y);
            chkAuto.AutoSize = true;
            Controls.Add(chkAuto);

            y += 30;
            chkLaunch = new CheckBox();
            chkLaunch.Text = "安装完成后立即运行";
            chkLaunch.Checked = true;
            chkLaunch.Location = new Point(26, y);
            chkLaunch.AutoSize = true;
            Controls.Add(chkLaunch);

            y += 44;
            progress = new ProgressBar();
            progress.Minimum = 0;
            progress.Maximum = 100;
            progress.Location = new Point(24, y);
            progress.Size = new Size(476, 18);
            progress.Style = ProgressBarStyle.Continuous;
            Controls.Add(progress);

            y += 26;
            lblStatus = new Label();
            lblStatus.Text = "";
            lblStatus.Location = new Point(26, y);
            lblStatus.Size = new Size(474, 20);
            Controls.Add(lblStatus);

            y += 34;
            btnInstall = new Button();
            btnInstall.Text = "安装";
            btnInstall.Size = new Size(110, 34);
            btnInstall.Location = new Point(300, y);
            btnInstall.BackColor = Color.FromArgb(76, 175, 80);
            btnInstall.ForeColor = Color.White;
            btnInstall.FlatStyle = FlatStyle.Flat;
            btnInstall.Click += (s, e) => DoInstall();
            Controls.Add(btnInstall);

            btnExit = new Button();
            btnExit.Text = "退出";
            btnExit.Size = new Size(110, 34);
            btnExit.Location = new Point(418, y);
            btnExit.Click += (s, e) => Close();
            Controls.Add(btnExit);
        }

        void Browse()
        {
            using (FolderBrowserDialog dlg = new FolderBrowserDialog())
            {
                dlg.Description = "选择安装目录";
                dlg.SelectedPath = txtPath.Text;
                if (dlg.ShowDialog(this) == DialogResult.OK)
                    txtPath.Text = dlg.SelectedPath;
            }
        }

        Image DrawBatteryIcon()
        {
            int s = 64;
            Bitmap bmp = new Bitmap(s, s);
            using (Graphics g = Graphics.FromImage(bmp))
            {
                g.SmoothingMode = SmoothingMode.AntiAlias;
                using (GraphicsPath path = new GraphicsPath())
                {
                    path.AddArc(8, 14, 16, 16, 180, 90);
                    path.AddLine(16, 14, 42, 14);
                    path.AddArc(40, 14, 16, 16, 270, 90);
                    path.AddLine(56, 22, 56, 38);
                    path.AddArc(40, 38, 16, 16, 0, 90);
                    path.AddLine(42, 54, 16, 54);
                    path.AddArc(8, 38, 16, 16, 90, 90);
                    path.CloseFigure();
                    g.FillPath(Brushes.White, path);
                }
                g.FillRectangle(Brushes.White, 56, 24, 5, 14);
                using (GraphicsPath fill = new GraphicsPath())
                {
                    fill.AddRectangle(new RectangleF(15, 20, 34, 22));
                    g.FillPath(Brushes.LimeGreen, fill);
                }
                using (Font f = new Font("Arial", 20, FontStyle.Bold))
                {
                    g.DrawString("100%", f, Brushes.White, 15, 22);
                }
            }
            return bmp;
        }

        void DoInstall()
        {
            if (done) { Close(); return; }

            string dest = txtPath.Text.Trim();
            if (dest.Length == 0)
            {
                MessageBox.Show(this, "请填写安装目录。", "bt-battery-bar 安装", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }
            try
            {
                dest = Path.GetFullPath(dest);
            }
            catch
            {
                MessageBox.Show(this, "安装目录无效。", "bt-battery-bar 安装", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            btnInstall.Enabled = false;
            btnExit.Enabled = false;
            txtPath.Enabled = false;

            InstallerCore core = new InstallerCore(dest, chkAuto.Checked, chkLaunch.Checked,
                (p, msg) =>
                {
                    progress.Value = Math.Min(p, 100);
                    lblStatus.Text = msg;
                    Application.DoEvents();
                });

            try
            {
                core.Install();
                done = true;
                btnInstall.Text = "完成";
                btnInstall.Enabled = true;
                btnExit.Enabled = true;
                MessageBox.Show(this,
                    "安装完成！\n\n已安装到：" + dest + "\n开机自启：" + (chkAuto.Checked ? "已启用" : "未启用"),
                    "bt-battery-bar 安装", MessageBoxButtons.OK, MessageBoxIcon.Information);
            }
            catch (Exception ex)
            {
                btnInstall.Enabled = true;
                btnExit.Enabled = true;
                MessageBox.Show(this, "安装失败：" + ex.Message, "bt-battery-bar 安装", MessageBoxButtons.OK, MessageBoxIcon.Error);
            }
        }
    }
}
