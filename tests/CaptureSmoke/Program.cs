using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Text.Json;

// 自分で作る試験窓だけを操作する。フォーカスが外れたら入力を中止する。
// 既存データのあるクリップボードは使用せず、試験が書き込んだデータだけを片付ける。
internal static class Program
{
    [StructLayout(LayoutKind.Sequential)] struct Input { public uint Type; public Union Data; }
    [StructLayout(LayoutKind.Explicit)] struct Union { [FieldOffset(0)] public Keyboard Key; [FieldOffset(0)] public Mouse Mouse; }
    [StructLayout(LayoutKind.Sequential)] struct Keyboard { public ushort Key, Scan; public uint Flags, Time; public nuint Extra; }
    [StructLayout(LayoutKind.Sequential)] struct Mouse { public int X,Y; public uint Data,Flags,Time; public nuint Extra; }
    [DllImport("user32.dll")] static extern uint SendInput(uint count, Input[] inputs, int size);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern nint GetForegroundWindow();
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern nint FindWindow(string? cls,string? title);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")] static extern nint GetClipboardOwner();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(nint window, out uint processId);
    static readonly List<string> results=[];
    static void Check(bool ok,string message) {if(!ok) throw new IOException(message);results.Add("PASS: "+message);Console.WriteLine(results[^1]);}
    static void Pump(int milliseconds) {var clock=Stopwatch.StartNew();while(clock.ElapsedMilliseconds<milliseconds){Application.DoEvents();Thread.Sleep(10);}}
    static bool Wait(Func<bool> condition,int milliseconds=5000) {var clock=Stopwatch.StartNew();do{Application.DoEvents();if(condition())return true;Thread.Sleep(15);}while(clock.ElapsedMilliseconds<milliseconds);return false;}
    static void Emit(Input input) {if(SendInput(1,[input],Marshal.SizeOf<Input>())!=1)throw new IOException("試験入力に失敗しました。");}
    static void Key(bool down)=>Emit(new(){Type=1,Data=new(){Key=new(){Key=0x77,Flags=down?0u:2u}}});
    static void Left(bool down)=>Emit(new(){Type=0,Data=new(){Mouse=new(){Flags=down?2u:4u}}});
    static JsonElement Send(string id,object? request=null)
    {
        var sid=WindowsIdentity.GetCurrent().User!.Value;
        using var pipe=new NamedPipeClientStream(".",$"KotorinToys.{sid}.{id}",PipeDirection.InOut,PipeOptions.Asynchronous|PipeOptions.CurrentUserOnly);
        using var timeout=new CancellationTokenSource(TimeSpan.FromSeconds(3));
        pipe.ConnectAsync(timeout.Token).GetAwaiter().GetResult();
        using var writer=new StreamWriter(pipe,leaveOpen:true){AutoFlush=true};using var reader=new StreamReader(pipe,leaveOpen:true);
        writer.WriteLine(request==null?"status":JsonSerializer.Serialize(request));
        using var reply=JsonDocument.Parse(reader.ReadLineAsync(timeout.Token).GetAwaiter().GetResult()??"");return reply.RootElement.Clone();
    }
    [STAThread] static int Main(string[] args)
    {
        ApplicationConfiguration.Initialize();
        Console.OutputEncoding = new UTF8Encoding(false);
        if(args.Length<2) {Console.Error.WriteLine("CaptureSmoke.exe <本体EXE> <版別ログ> [開始ゲート]");return 2;}
        var logs=Path.GetFullPath(args[1]);Directory.CreateDirectory(logs);
        var settings=Path.Combine(logs,"settings");Directory.CreateDirectory(settings);
        File.WriteAllText(Path.Combine(settings,"config.json"),"{\"first_launch_done\":true,\"text_capture_key\":\"F8\",\"perf_log\":true}");
        var id="focus-translator-"+Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(settings))).ToLowerInvariant()[..16];
        Process? process=null;bool held=false,mouseHeld=false,clipboardChanged=false;Form? fixture=null;
        try
        {
            var start=new ProcessStartInfo(Path.GetFullPath(args[0])){UseShellExecute=false};
            foreach(var arg in new[]{"--startup","--settings-dir",settings})start.ArgumentList.Add(arg);
            process=Process.Start(start)??throw new IOException("試験プロセスを開始できません。");
            Check(Wait(()=>{try{return Send(id).GetProperty("ready").GetBoolean();}catch{return false;}},30000),"専用設定の実EXEが準備完了");
            fixture=new Form{Text="FocusTranslator capture smoke",BackColor=Color.White,StartPosition=FormStartPosition.Manual,KeyPreview=true,TopMost=true};
            var work=Screen.PrimaryScreen!.WorkingArea;fixture.Bounds=new(work.Left+work.Width/2,work.Top,work.Width/2,work.Height);
            var label=new Label{Text="Focus capture alpha",Font=new Font("Segoe UI",20),AutoSize=true,Location=new(40,80)};fixture.Controls.Add(label);
            using var bitmap=new Bitmap(620,140);using(var g=Graphics.FromImage(bitmap)){g.Clear(Color.White);using var font=new Font("Segoe UI",20);g.DrawString("First range line",font,Brushes.Black,10,10);g.DrawString("Second range line",font,Brushes.Black,10,65);}
            var picture=new PictureBox{Image=bitmap,Location=new(40,170),Size=bitmap.Size};fixture.Controls.Add(picture);
            int clicks=0,keys=0;picture.MouseDown+=(_,_)=>clicks++;label.MouseDown+=(_,_)=>clicks++;fixture.KeyDown+=(_,e)=>{if(e.KeyCode==Keys.F8)keys++;};
            fixture.Show();fixture.Activate();Pump(300);
            File.WriteAllText(Path.Combine(logs,"ready.txt"),fixture.Handle.ToString());
            if(args.Length==3)Check(Wait(()=>File.Exists(args[2]),60000),"録画開始ゲートを受信");
            fixture.BringToFront();fixture.Activate();Pump(100);
            void Focused(){if(GetForegroundWindow()!=fixture.Handle)throw new IOException("試験窓以外が前面になったため入力を中止しました。");}
            void PointAt(int x,int y){Focused();var point=fixture.PointToScreen(new(x,y));if(!SetCursorPos(point.X,point.Y))throw new IOException("試験座標への移動に失敗しました。");Pump(40);}
            if (Clipboard.GetDataObject()?.GetFormats(false).Any(format => format != "DataObject") == true)
                throw new IOException("クリップボードに既存データがあるため中止しました。");
            clipboardChanged=true;Clipboard.SetText("focus-capture-sentinel");
            var labelCenter = new Point(label.Left + label.Width / 2, label.Top + label.Height / 2);
            var blank = new Point(Math.Min(fixture.ClientSize.Width - 20, 760), 430);
            PointAt(labelCenter.X,labelCenter.Y);Key(true);held=true;
            Check(Wait(()=>IsWindowVisible(FindWindow("FocusTranslatorDetect",null))),"押下中にフォーカス枠を表示");Pump(700);
            Check(Clipboard.GetText()=="focus-capture-sentinel","キー押下中はクリップボードを変更しない");
            Key(false);held=false;
            Check(Wait(()=>Clipboard.ContainsText()&&Clipboard.GetText().Contains("Focus capture alpha")),"キー解放でカーソル位置の文字を自動コピー");
            var copied=Clipboard.GetText();
            PointAt(labelCenter.X,labelCenter.Y);Key(true);held=true;Pump(500);PointAt(blank.X,blank.Y);Key(false);held=false;Pump(1400);
            Check(Clipboard.GetText()==copied,"文字から余白へ移動して解放しても以前の文字を誤コピーしない");
            PointAt(blank.X,blank.Y);Key(true);held=true;Pump(350);Key(false);held=false;Pump(1200);
            Check(Clipboard.GetText()==copied,"何もない場所で解放した場合はクリップボードを保持");
            PointAt(42,174);Key(true);held=true;Pump(150);Left(true);mouseHeld=true;Pump(150);
            PointAt(500,300);Left(false);mouseHeld=false;Pump(200);Key(false);held=false;
            Check(Wait(()=>{var value=Clipboard.GetText();return value.Contains("First range line")&&value.Contains("Second range line");}),"押下中の左ドラッグ範囲から複数行をコピー");
            Check(clicks==0,"キャプチャ用の左クリックを背後の試験画面へ渡さない");
            Check(keys==0,"専用キャプチャキーを背後の試験画面へ渡さない");
            Pump(400);Console.WriteLine($"PASS: {results.Count} 件");return 0;
        }
        catch(Exception error){results.Add("FAIL: "+error);Console.Error.WriteLine(error);return 1;}
        finally
        {
            if(held)Key(false);if(mouseHeld)Left(false);Pump(500);
            if (clipboardChanged)
            {
                GetWindowThreadProcessId(GetClipboardOwner(), out uint owner);
                if (owner == (uint)Environment.ProcessId || (process is not null && owner == (uint)process.Id))
                {
                    try { Clipboard.Clear(); results.Add("PASS: 試験用クリップボードを消去"); }
                    catch (ExternalException) { results.Add("FAIL: 試験用クリップボードを消去できませんでした。"); }
                }
            }
            if(process is {HasExited:false})try{var reply=Send(id,new{command="stop",expectedPid=process.Id});if(reply.GetProperty("result").GetString()=="accepted")process.WaitForExit(5000);}catch(Exception error){results.Add("停止未確認: "+error.Message);}
            fixture?.Dispose();process?.Dispose();File.WriteAllLines(Path.Combine(logs,"result.txt"),results);
        }
    }
}
