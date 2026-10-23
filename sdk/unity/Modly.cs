// Modly SDK для Unity и любых .NET-игр — один файл, без зависимостей.
//
// Опишите переключатели, ползунки и кнопки своего мода — Modly нарисует их в своём окне и будет
// присылать изменения. Мод работает и без Modly: пока Modly не запущен, SDK тихо ждёт в фоне.
//
//   var modly = new ModlyClient("my-mod", "My Mod", "1.0.0");
//   var s = modly.Page("main", new Txt("Main").Ru("Главное"), "star").Section("player", "Player");
//   s.Toggle("god", "God mode", () => god, v => god = v).Icon("shield");
//   s.Slider("speed", "Speed", 0.5f, 5f, () => speed, v => speed = v).Step(0.1);
//   s.Button("heal", "Heal", Heal).Tone(Tone.Accent);
//   modly.Start();
//   // каждый кадр, на главном потоке (например, в MonoBehaviour.Update):
//   modly.Update();
//
// Совместимость: C# 7.3, .NET 3.5+ (Unity Mono, IL2CPP, BepInEx, MelonLoader). Канал открывается через
// kernel32, а не System.IO.Pipes, — поэтому SDK работает и в играх с урезанными сборками .NET.
// Типы SDK — internal: файл вшивается в каждый мод, и несколько модов с SDK в одной игре не мешают друг другу.
// Протокол — docs/PROTOCOL.md в репозитории Modly.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Modly
{
    /// <summary>Оттенок кнопки, показателя, уведомления.</summary>
    internal enum Tone { Default, Muted, Accent, Info, Success, Warning, Danger }

    /// <summary>Текст с переводами: <c>new Txt("Speed").Ru("Скорость")</c>. Строка превращается в Txt сама.</summary>
    internal sealed class Txt
    {
        private readonly string _default;
        private Dictionary<string, string> _tr;

        public Txt(string text)
        {
            _default = text ?? "";
        }

        /// <summary>Сначала по-русски, потом по-английски: <c>Txt.Of("Скорость", "Speed")</c>.</summary>
        public static Txt Of(string ru, string en)
        {
            return new Txt(en).Ru(ru);
        }

        public Txt Ru(string text)
        {
            return Lang("ru", text);
        }

        public Txt Lang(string code, string text)
        {
            if (_tr == null)
            {
                _tr = new Dictionary<string, string>();
            }
            _tr[code] = text ?? "";
            return this;
        }

        public string Get(string lang)
        {
            string s;
            if (_tr != null && lang != null && _tr.TryGetValue(lang, out s))
            {
                return s;
            }
            return _default;
        }

        public static implicit operator Txt(string s)
        {
            return s == null ? null : new Txt(s);
        }

        internal void Write(Json.Writer w)
        {
            if (_tr == null)
            {
                w.String(_default);
                return;
            }
            w.BeginObject();
            w.Key("en").String(_default);
            foreach (KeyValuePair<string, string> kv in _tr)
            {
                w.Key(kv.Key).String(kv.Value);
            }
            w.EndObject();
        }

        internal string Signature()
        {
            var w = new Json.Writer();
            Write(w);
            return w.ToString();
        }
    }

    /// <summary>Вариант выбора.</summary>
    internal sealed class Option
    {
        public string Value;
        public Txt Label;
        public string Icon;

        public Option(string value, Txt label, string icon = null)
        {
            Value = value;
            Label = label;
            Icon = icon;
        }

        public static Option Of(string value, Txt label, string icon = null)
        {
            return new Option(value, label, icon);
        }

        internal void Write(Json.Writer w)
        {
            w.BeginObject();
            w.Key("value").String(Value);
            if (Label != null)
            {
                w.Key("label");
                Label.Write(w);
            }
            if (Icon != null)
            {
                w.Key("icon").String(Icon);
            }
            w.EndObject();
        }
    }

    /// <summary>Элемент интерфейса. Настраивается цепочкой: <c>.Icon("shield").Describe("...")</c>.</summary>
    internal sealed class Control
    {
        internal readonly ModlyClient Client;
        internal readonly string Id;
        internal readonly string Kind;
        internal Txt Label;
        internal Txt Description;
        internal string IconName;
        internal Tone ToneValue;
        internal Txt ConfirmText;
        internal double? Min, Max, StepValue;
        internal string UnitText;
        internal double[] PresetValues;
        internal Txt PlaceholderText;
        internal string HotkeyText;
        internal Option[] OptionList;
        internal bool Multiple;
        internal Func<object> Getter;
        internal Action<object> Setter;
        internal Action Action;
        internal Func<string> ActionWithText;
        internal Func<bool> EnabledFunc;
        internal Func<bool> VisibleFunc;
        internal Func<Option[]> OptionsFunc;
        internal bool Enabled = true;
        internal bool Visible = true;
        internal string SentValue;
        internal string SentOptions;

        internal Control(ModlyClient client, string id, string kind, Txt label)
        {
            Client = client;
            Id = id;
            Kind = kind;
            Label = label;
        }

        public Control Icon(string name) { IconName = name; return this; }
        public Control Describe(Txt text) { Description = text; return this; }
        public Control Tone(Tone tone) { ToneValue = tone; return this; }
        /// <summary>Кнопка сработает только со второго нажатия (для опасных действий).</summary>
        public Control Confirm(Txt text) { ConfirmText = text ?? new Txt(""); return this; }
        public Control Step(double step) { StepValue = step; return this; }
        public Control Unit(string unit) { UnitText = unit; return this; }
        public Control Range(double min, double max) { Min = min; Max = max; return this; }
        public Control Presets(params double[] values) { PresetValues = values; return this; }
        public Control Placeholder(Txt text) { PlaceholderText = text; return this; }
        public Control Hotkey(string key) { HotkeyText = key; return this; }
        /// <summary>Доступность элемента пересчитывается сама (например, «пока мир не загружен — нельзя»).</summary>
        public Control EnabledWhen(Func<bool> when) { EnabledFunc = when; return this; }
        public Control VisibleWhen(Func<bool> when) { VisibleFunc = when; return this; }
        /// <summary>Варианты выбора, которые меняются по ходу игры.</summary>
        public Control OptionsFrom(Func<Option[]> options) { OptionsFunc = options; return this; }

        /// <summary>Поменять подпись прямо сейчас.</summary>
        public void SetLabel(Txt label)
        {
            Label = label;
            Client.SendPatch(this, "label");
        }

        internal void WriteLayout(Json.Writer w, string value)
        {
            w.BeginObject();
            w.Key("id").String(Id);
            w.Key("kind").String(Kind);
            if (Label != null) { w.Key("label"); Label.Write(w); }
            if (Description != null) { w.Key("description"); Description.Write(w); }
            if (IconName != null) w.Key("icon").String(IconName);
            if (!Enabled) w.Key("enabled").Bool(false);
            if (!Visible) w.Key("visible").Bool(false);
            if (HotkeyText != null) w.Key("hotkey").String(HotkeyText);
            if (ToneValue != Modly.Tone.Default) w.Key("tone").String(ToneName(ToneValue));
            if (ConfirmText != null) { w.Key("confirm"); ConfirmText.Write(w); }
            if (Min.HasValue) w.Key("min").Number(Min.Value);
            if (Max.HasValue) w.Key("max").Number(Max.Value);
            if (StepValue.HasValue) w.Key("step").Number(StepValue.Value);
            if (UnitText != null) w.Key("unit").String(UnitText);
            if (PlaceholderText != null) { w.Key("placeholder"); PlaceholderText.Write(w); }
            if (PresetValues != null)
            {
                w.Key("presets").BeginArray();
                foreach (double p in PresetValues) w.Number(p);
                w.EndArray();
            }
            if (Kind == "select")
            {
                if (Multiple) w.Key("multiple").Bool(true);
                w.Key("options");
                WriteOptions(w);
            }
            if (value != null) w.Key("value").Raw(value);
            w.EndObject();
        }

        internal void WriteOptions(Json.Writer w)
        {
            w.BeginArray();
            if (OptionList != null)
            {
                foreach (Option o in OptionList) o.Write(w);
            }
            w.EndArray();
        }

        internal string ReadValue()
        {
            if (Getter == null) return null;
            object v = Getter();
            var w = new Json.Writer();
            w.Value(v);
            return w.ToString();
        }

        internal static string ToneName(Tone t)
        {
            switch (t)
            {
                case Modly.Tone.Muted: return "muted";
                case Modly.Tone.Accent: return "accent";
                case Modly.Tone.Info: return "info";
                case Modly.Tone.Success: return "success";
                case Modly.Tone.Warning: return "warning";
                case Modly.Tone.Danger: return "danger";
                default: return "default";
            }
        }
    }

    internal sealed class ModlyPage
    {
        internal readonly ModlyClient Client;
        internal readonly string Id;
        internal readonly Txt Title;
        internal readonly string Icon;
        internal readonly List<ModlySection> Sections = new List<ModlySection>();

        internal ModlyPage(ModlyClient client, string id, Txt title, string icon)
        {
            Client = client;
            Id = id;
            Title = title;
            Icon = icon;
        }

        public ModlySection Section(string id, Txt title = null)
        {
            foreach (ModlySection s in Sections)
            {
                if (s.Id == id) return s;
            }
            var section = new ModlySection(this, id, title);
            Sections.Add(section);
            return section;
        }
    }

    internal sealed class ModlySection
    {
        internal readonly ModlyPage Page;
        internal readonly string Id;
        internal readonly Txt Title;
        internal Txt DescriptionText;
        internal readonly List<Control> Controls = new List<Control>();

        internal ModlySection(ModlyPage page, string id, Txt title)
        {
            Page = page;
            Id = id;
            Title = title;
        }

        public ModlySection Describe(Txt text)
        {
            DescriptionText = text;
            return this;
        }

        private Control Add(string id, string kind, Txt label)
        {
            var c = new Control(Page.Client, id, kind, label);
            Controls.Add(c);
            Page.Client.Register(c);
            return c;
        }

        /// <summary>Переключатель.</summary>
        public Control Toggle(string id, Txt label, Func<bool> get, Action<bool> set)
        {
            Control c = Add(id, "toggle", label);
            c.Getter = () => get();
            c.Setter = v => set(Json.ToBool(v));
            return c;
        }

        /// <summary>Кнопка.</summary>
        public Control Button(string id, Txt label, Action action)
        {
            Control c = Add(id, "button", label);
            c.Action = action;
            return c;
        }

        /// <summary>Кнопка, которая сообщает итог: вернула текст — Modly покажет его уведомлением.</summary>
        public Control Button(string id, Txt label, Func<string> action)
        {
            Control c = Add(id, "button", label);
            c.ActionWithText = action;
            return c;
        }

        /// <summary>Подпись на кнопке, которая меняется по ходу игры («Призвать 5»).</summary>
        public Control Button(string id, Txt label, Func<string> caption, Action action)
        {
            Control c = Add(id, "button", label);
            c.Getter = () => caption();
            c.Action = action;
            return c;
        }

        /// <summary>Ползунок.</summary>
        public Control Slider(string id, Txt label, float min, float max, Func<float> get, Action<float> set)
        {
            Control c = Add(id, "slider", label);
            c.Min = min;
            c.Max = max;
            c.Getter = () => get();
            c.Setter = v => set((float)Json.ToDouble(v));
            return c;
        }

        /// <summary>Ползунок для целых чисел.</summary>
        public Control Slider(string id, Txt label, int min, int max, Func<int> get, Action<int> set)
        {
            Control c = Add(id, "slider", label);
            c.Min = min;
            c.Max = max;
            c.StepValue = 1;
            c.Getter = () => get();
            c.Setter = v => set((int)Math.Round(Json.ToDouble(v)));
            return c;
        }

        /// <summary>Целое число с кнопками −/+.</summary>
        public Control Number(string id, Txt label, Func<int> get, Action<int> set)
        {
            Control c = Add(id, "number", label);
            c.StepValue = 1;
            c.Getter = () => get();
            c.Setter = v => set((int)Math.Round(Json.ToDouble(v)));
            return c;
        }

        /// <summary>Дробное число с кнопками −/+.</summary>
        public Control Number(string id, Txt label, Func<double> get, Action<double> set)
        {
            Control c = Add(id, "number", label);
            c.Getter = () => get();
            c.Setter = v => set(Json.ToDouble(v));
            return c;
        }

        /// <summary>Текстовое поле (значение приходит по Enter).</summary>
        public Control Text(string id, Txt label, Func<string> get, Action<string> set)
        {
            Control c = Add(id, "text", label);
            c.Getter = () => get();
            c.Setter = v => set(Json.ToStr(v));
            return c;
        }

        /// <summary>Выбор одного варианта.</summary>
        public Control Select(string id, Txt label, Option[] options, Func<string> get, Action<string> set)
        {
            Control c = Add(id, "select", label);
            c.OptionList = options;
            c.Getter = () => get();
            c.Setter = v => set(Json.ToStr(v));
            return c;
        }

        /// <summary>Выбор нескольких вариантов.</summary>
        public Control MultiSelect(string id, Txt label, Option[] options, Func<string[]> get, Action<string[]> set)
        {
            Control c = Add(id, "select", label);
            c.OptionList = options;
            c.Multiple = true;
            c.Getter = () => get();
            c.Setter = v => set(Json.ToStrings(v));
            return c;
        }

        /// <summary>Показатель только для чтения: число или строка.</summary>
        public Control Stat(string id, Txt label, Func<object> get)
        {
            Control c = Add(id, "stat", label);
            c.Getter = get;
            return c;
        }

        /// <summary>Полоса прогресса, 0..1.</summary>
        public Control Progress(string id, Txt label, Func<float> get)
        {
            Control c = Add(id, "progress", label);
            c.Getter = () => get();
            return c;
        }

        /// <summary>Поясняющий текст.</summary>
        public Control Note(string id, Txt text)
        {
            return Add(id, "note", text);
        }

        public Control Separator(string id)
        {
            return Add(id, "separator", null);
        }
    }

    // =========================================================================================
    // Карты

    /// <summary>Где сработало действие на карте: у метки (Marker — её id) или по клику в пустое место
    /// (Marker = null). X, Y — в координатах игры (тех же, что края карты и метки).</summary>
    internal sealed class MapHit
    {
        public string Marker;
        public double X;
        public double Y;
    }

    /// <summary>Действие на карте: кнопка в подписи метки («Телепорт») или по клику в пустое место.</summary>
    internal sealed class MapAction
    {
        internal readonly string Id;
        internal readonly Txt Label;
        internal Tone ToneValue;
        internal Txt ConfirmText;
        internal string IconName;

        public MapAction(string id, Txt label)
        {
            Id = id;
            Label = label;
        }

        public MapAction Tone(Tone tone) { ToneValue = tone; return this; }
        /// <summary>Сработает только со второго нажатия.</summary>
        public MapAction Confirm(Txt text) { ConfirmText = text ?? new Txt(""); return this; }
        public MapAction Icon(string name) { IconName = name; return this; }

        internal void Write(Json.Writer w)
        {
            w.BeginObject();
            w.Key("id").String(Id);
            if (Label != null) { w.Key("label"); Label.Write(w); }
            if (ToneValue != Modly.Tone.Default) w.Key("tone").String(Control.ToneName(ToneValue));
            if (ConfirmText != null) { w.Key("confirm"); ConfirmText.Write(w); }
            if (IconName != null) w.Key("icon").String(IconName);
            w.EndObject();
        }
    }

    /// <summary>Категория меток: её можно скрыть на карте.</summary>
    internal sealed class MapCategory
    {
        internal readonly ModlyMap Map;
        internal readonly string Id;
        internal readonly Txt Label;
        internal string IconName;
        internal string ColorHex;
        internal bool IsHidden;

        internal MapCategory(ModlyMap map, string id, Txt label)
        {
            Map = map;
            Id = id;
            Label = label;
        }

        public MapCategory Icon(string name) { IconName = name; Map.Dirty = true; return this; }
        /// <summary>Цвет <c>"#rrggbb"</c>.</summary>
        public MapCategory Color(string hex) { ColorHex = hex; Map.Dirty = true; return this; }
        /// <summary>Скрыта, пока игрок её не включит.</summary>
        public MapCategory Hidden() { IsHidden = true; Map.Dirty = true; return this; }

        internal void Write(Json.Writer w)
        {
            w.BeginObject();
            w.Key("id").String(Id);
            if (Label != null) { w.Key("label"); Label.Write(w); }
            if (IconName != null) w.Key("icon").String(IconName);
            if (ColorHex != null) w.Key("color").String(ColorHex);
            if (IsHidden) w.Key("hidden").Bool(true);
            w.EndObject();
        }
    }

    /// <summary>Метка на карте. Настраивается цепочкой: <c>.Cat("places").Title("Дом").Action("tp", "Телепорт")</c>.
    /// Изменения после отправки SDK сам присылает Modly.</summary>
    internal sealed class MapMarker
    {
        internal readonly ModlyMap Map;
        internal readonly string Id;
        internal double X, Y;
        internal string CatId;
        internal Txt TitleText;
        internal Txt DescriptionText;
        internal string IconName;
        internal string ColorHex;
        internal bool IsDot;
        internal readonly List<MapAction> Actions = new List<MapAction>();
        // живая метка: место спрашивается само
        internal Func<double> LiveX, LiveY;
        internal bool Changed;
        // последнее присланное место (double.NaN в урезанных сборках игр бывает вырезан — поэтому флаг)
        internal double SentX, SentY;
        internal bool Sent;

        internal MapMarker(ModlyMap map, string id, double x, double y)
        {
            Map = map;
            Id = id;
            X = x;
            Y = y;
        }

        private MapMarker Touch() { Changed = true; return this; }

        public MapMarker Cat(string id) { CatId = id; return Touch(); }
        public MapMarker Title(Txt text) { TitleText = text; return Touch(); }
        public MapMarker Text(Txt text) { DescriptionText = text; return Touch(); }
        public MapMarker Icon(string name) { IconName = name; return Touch(); }
        /// <summary>Цвет <c>"#rrggbb"</c> (иначе — цвет категории).</summary>
        public MapMarker Color(string hex) { ColorHex = hex; return Touch(); }
        /// <summary>Точка вместо булавки — для того, что движется: игрок, питомец.</summary>
        public MapMarker Dot() { IsDot = true; return Touch(); }
        /// <summary>Кнопка в подписи метки; что делать — <see cref="ModlyMap.On(string, Func{MapHit, string})"/>.</summary>
        public MapMarker Action(string id, Txt label) { Actions.Add(new MapAction(id, label)); return Touch(); }
        public MapMarker Action(MapAction action) { Actions.Add(action); return Touch(); }
        /// <summary>Передвинуть метку.</summary>
        public MapMarker MoveTo(double x, double y) { X = x; Y = y; return Touch(); }

        internal void Write(Json.Writer w, double x, double y)
        {
            w.BeginObject();
            w.Key("id").String(Id);
            w.Key("x").Number(x);
            w.Key("y").Number(y);
            if (CatId != null) w.Key("cat").String(CatId);
            if (TitleText != null) { w.Key("title"); TitleText.Write(w); }
            if (DescriptionText != null) { w.Key("text"); DescriptionText.Write(w); }
            if (IconName != null) w.Key("icon").String(IconName);
            if (ColorHex != null) w.Key("color").String(ColorHex);
            if (IsDot) w.Key("style").String("dot");
            if (Actions.Count > 0)
            {
                w.Key("actions").BeginArray();
                foreach (MapAction a in Actions) a.Write(w);
                w.EndArray();
            }
            w.EndObject();
        }
    }

    /// <summary>Карта мода: картинка игры, её края в координатах игры, категории и метки.
    /// Создаётся <see cref="ModlyClient.Map"/>.</summary>
    internal sealed class ModlyMap
    {
        internal readonly ModlyClient Client;
        internal readonly string Id;
        internal Txt TitleText;
        internal string Image;
        internal bool HasBounds;
        internal double Left, Top, Right, Bottom;
        internal string FocusId;
        internal readonly List<MapCategory> Categories = new List<MapCategory>();
        internal readonly List<MapMarker> Markers = new List<MapMarker>();
        internal readonly List<MapAction> ClickActions = new List<MapAction>();
        internal readonly Dictionary<string, Func<MapHit, string>> Handlers = new Dictionary<string, Func<MapHit, string>>();
        internal readonly List<string> Removed = new List<string>();
        // прислать карту целиком: новое подключение или изменилось её описание
        internal bool Dirty = true;

        internal ModlyMap(ModlyClient client, string id, Txt title, string image)
        {
            Client = client;
            Id = id;
            TitleText = title;
            Image = image;
        }

        /// <summary>Края картинки в координатах игры. Без них координаты меток — пиксели картинки.
        /// Если y игры растёт вверх, top больше bottom.</summary>
        public ModlyMap Bounds(double left, double top, double right, double bottom)
        {
            HasBounds = true;
            Left = left; Top = top; Right = right; Bottom = bottom;
            Dirty = true;
            return this;
        }

        public MapCategory Category(string id, Txt label)
        {
            Categories.RemoveAll(c => c.Id == id);
            var c2 = new MapCategory(this, id, label);
            Categories.Add(c2);
            Dirty = true;
            return c2;
        }

        /// <summary>Метка (тот же id заменяет прежнюю).</summary>
        public MapMarker Marker(string id, double x, double y)
        {
            Markers.RemoveAll(m => m.Id == id);
            Removed.Remove(id);
            var m2 = new MapMarker(this, id, x, y) { Changed = true };
            Markers.Add(m2);
            return m2;
        }

        /// <summary>Живая метка: место берётся из x() и y() и само присылается Modly, когда меняется.</summary>
        public MapMarker Live(string id, Func<double> x, Func<double> y)
        {
            MapMarker m = Marker(id, 0, 0);
            m.LiveX = x;
            m.LiveY = y;
            return m;
        }

        /// <summary>Действие по клику в пустое место карты («Телепорт сюда»).</summary>
        public MapAction ClickAction(string id, Txt label)
        {
            ClickActions.RemoveAll(a => a.Id == id);
            var a2 = new MapAction(id, label);
            ClickActions.Add(a2);
            Dirty = true;
            return a2;
        }

        /// <summary>Что делать по действию: у любой метки с этим действием и по клику в пустое место.
        /// Вернули текст — Modly покажет его уведомлением; исключение — ошибкой.</summary>
        public ModlyMap On(string actionId, Func<MapHit, string> handler)
        {
            Handlers[actionId] = handler;
            return this;
        }

        public ModlyMap On(string actionId, Action<MapHit> handler)
        {
            Handlers[actionId] = hit => { handler(hit); return null; };
            return this;
        }

        /// <summary>Метка, к которой карта подъезжает кнопкой «Где я» (обычно игрок).</summary>
        public ModlyMap Focus(string markerId)
        {
            FocusId = markerId;
            Dirty = true;
            return this;
        }

        /// <summary>Убрать метку.</summary>
        public void Remove(string markerId)
        {
            if (Markers.RemoveAll(m => m.Id == markerId) > 0 && !Removed.Contains(markerId))
            {
                Removed.Add(markerId);
            }
        }

        public MapMarker Find(string markerId)
        {
            foreach (MapMarker m in Markers)
            {
                if (m.Id == markerId) return m;
            }
            return null;
        }

        // место метки сейчас (у живой — спросить)
        internal bool Position(MapMarker m, out double x, out double y)
        {
            x = m.X;
            y = m.Y;
            if (m.LiveX == null || m.LiveY == null) return true;
            try
            {
                x = m.LiveX();
                y = m.LiveY();
                return true;
            }
            catch (Exception ex)
            {
                Client.LogError(ex);
                return false;
            }
        }

        // вся карта одной строкой
        internal string Build()
        {
            var w = ModlyClient.Message("map");
            w.Key("id").String(Id);
            if (TitleText != null) { w.Key("title"); TitleText.Write(w); }
            w.Key("image").String(Image ?? "");
            if (HasBounds)
            {
                w.Key("bounds").BeginObject();
                w.Key("left").Number(Left);
                w.Key("top").Number(Top);
                w.Key("right").Number(Right);
                w.Key("bottom").Number(Bottom);
                w.EndObject();
            }
            w.Key("categories").BeginArray();
            foreach (MapCategory c in Categories) c.Write(w);
            w.EndArray();
            w.Key("markers").BeginArray();
            foreach (MapMarker m in Markers)
            {
                double x, y;
                if (!Position(m, out x, out y)) continue;
                m.SentX = x;
                m.Sent = true;
                m.SentY = y;
                m.Changed = false;
                m.Write(w, x, y);
            }
            w.EndArray();
            if (ClickActions.Count > 0)
            {
                w.Key("click_actions").BeginArray();
                foreach (MapAction a in ClickActions) a.Write(w);
                w.EndArray();
            }
            if (FocusId != null) w.Key("focus").String(FocusId);
            w.EndObject();
            Removed.Clear();
            return w.ToString();
        }

        // изменения с прошлого раза (метки и живые точки) или null
        internal string BuildPatch()
        {
            Json.Writer w = null;
            foreach (MapMarker m in Markers)
            {
                double x, y;
                if (!Position(m, out x, out y)) continue;
                if (!m.Changed && m.Sent && x == m.SentX && y == m.SentY) continue;
                if (w == null)
                {
                    w = ModlyClient.Message("map_patch");
                    w.Key("map").String(Id);
                    w.Key("set").BeginArray();
                }
                m.SentX = x;
                m.Sent = true;
                m.SentY = y;
                m.Changed = false;
                m.Write(w, x, y);
            }
            if (w == null && Removed.Count == 0) return null;
            if (w == null)
            {
                w = ModlyClient.Message("map_patch");
                w.Key("map").String(Id);
            }
            else
            {
                w.EndArray();
            }
            if (Removed.Count > 0)
            {
                w.Key("remove").BeginArray();
                foreach (string id in Removed) w.String(id);
                w.EndArray();
                Removed.Clear();
            }
            w.EndObject();
            return w.ToString();
        }
    }

    /// <summary>Соединение с Modly и интерфейс мода.</summary>
    internal sealed class ModlyClient : IDisposable
    {
        public const string PipeName = @"\\.\pipe\modly";
        public const string SdkName = "modly-unity/1.0.0";

        public readonly string Id;
        public readonly string Name;
        public readonly string Version;
        public string Author { get; set; }
        public Txt Description { get; set; }
        public string Url { get; set; }
        public string GameId { get; set; }
        public string GameName { get; set; }
        public string GameExe { get; set; }
        /// <summary>Имя канала Modly (задавать до Start). Менять нужно только для тестов.</summary>
        public string Pipe { get; set; } = PipeName;
        /// <summary>Как часто проверять изменившиеся значения, секунд.</summary>
        public float SyncInterval { get; set; } = 0.2f;

        /// <summary>Язык Modly: "ru", "en"… (пока не подключились — null).</summary>
        public string Language { get; private set; }
        public bool Connected { get { return _connected; } }
        /// <summary>Сменился язык Modly.</summary>
        public event Action<string> LanguageChanged;
        /// <summary>Подключились к Modly (интерфейс уже отправлен).</summary>
        public event Action ConnectedToModly;
        /// <summary>Связь с Modly пропала (Modly закрыт).</summary>
        public event Action DisconnectedFromModly;

        private readonly List<ModlyPage> _pages = new List<ModlyPage>();
        private readonly Dictionary<string, Control> _controls = new Dictionary<string, Control>();
        private readonly List<ModlyMap> _maps = new List<ModlyMap>();
        private readonly Queue<string> _outgoing = new Queue<string>();
        private readonly Queue<string> _incoming = new Queue<string>();
        private Thread _thread;
        private volatile bool _stop;
        private volatile bool _connected;
        private volatile int _generation;
        private int _sentGeneration;
        private bool _wasConnected;
        private volatile int _lastPump;
        // интерфейс, собранный заранее (пока Modly нет): отправляется сразу при подключении, даже если игра на паузе
        private volatile string _cachedLayout;
        private int _cachedAt;
        private bool _pausedSent;
        private int _lastSync; // Environment.TickCount: Stopwatch в урезанных сборках игр бывает вырезан
        private string _helloLine;

        public ModlyClient(string id, string name, string version)
        {
            Id = id;
            Name = name;
            Version = version;
        }

        public ModlyPage Page(string id, Txt title, string icon = null)
        {
            foreach (ModlyPage p in _pages)
            {
                if (p.Id == id) return p;
            }
            var page = new ModlyPage(this, id, title, icon);
            _pages.Add(page);
            return page;
        }

        internal void Register(Control c)
        {
            _controls[c.Id] = c;
        }

        /// <summary>Карта мода. image — полный путь к PNG, JPEG или WebP на этом компьютере
        /// (<c>C:\…\map.png</c>) или <see cref="DataUrl"/>. Тот же id даёт ту же карту.</summary>
        public ModlyMap Map(string id, Txt title, string image)
        {
            foreach (ModlyMap m in _maps)
            {
                if (m.Id == id)
                {
                    m.TitleText = title;
                    m.Image = image;
                    m.Dirty = true;
                    return m;
                }
            }
            var map = new ModlyMap(this, id, title, image);
            _maps.Add(map);
            return map;
        }

        /// <summary>Убрать карту целиком.</summary>
        public void RemoveMap(string id)
        {
            if (_maps.RemoveAll(m => m.Id == id) > 0)
            {
                var w = Message("map_remove");
                w.Key("map").String(id);
                Send(w);
            }
        }

        /// <summary>Картинка, вшитая в мод, — data:-адрес для <see cref="Map"/> (до ~3 МБ: предел сообщения — 4 МиБ).</summary>
        public static string DataUrl(string mime, byte[] bytes)
        {
            return "data:" + mime + ";base64," + Convert.ToBase64String(bytes);
        }

        /// <summary>Начать ждать Modly в фоне.</summary>
        public void Start()
        {
            if (_thread != null) return;
            var w = new Json.Writer();
            w.BeginObject();
            w.Key("type").String("hello");
            w.Key("protocol").Number(1);
            w.Key("mod").BeginObject();
            w.Key("id").String(Id);
            w.Key("name").String(Name);
            w.Key("version").String(Version ?? "");
            if (Author != null) w.Key("author").String(Author);
            if (Description != null) { w.Key("description"); Description.Write(w); }
            if (Url != null) w.Key("url").String(Url);
            w.EndObject();
            if (GameId != null || GameName != null)
            {
                w.Key("game").BeginObject();
                w.Key("id").String(GameId ?? "");
                w.Key("name").String(GameName ?? "");
                if (GameExe != null) w.Key("exe").String(GameExe);
                w.EndObject();
            }
            w.Key("pid").Number(Native.GetCurrentProcessId());
            w.Key("sdk").String(SdkName);
            w.EndObject();
            _helloLine = w.ToString() + "\n";
            _thread = new Thread(IoLoop) { IsBackground = true, Name = "Modly" };
            _thread.Start();
        }

        /// <summary>Каждый кадр на главном потоке: команды Modly и рассылка изменившихся значений.</summary>
        public void Update()
        {
            if (_thread == null) return;
            _lastPump = Environment.TickCount;
            if (_wasConnected && !_connected)
            {
                _wasConnected = false;
                Action lost = DisconnectedFromModly;
                if (lost != null) Safe(lost);
            }
            if (_connected && _generation != _sentGeneration)
            {
                _sentGeneration = _generation;
                SendLayout();
                foreach (ModlyMap m in _maps) m.Dirty = true;
                SyncMaps();
                _wasConnected = true;
                Action handler = ConnectedToModly;
                if (handler != null) Safe(handler);
            }
            if (!_connected && (_cachedLayout == null || unchecked(Environment.TickCount - _cachedAt) > 2000))
            {
                _cachedAt = Environment.TickCount;
                try { _cachedLayout = BuildLayout(false); } catch (Exception) { }
            }
            while (true)
            {
                string line;
                lock (_incoming)
                {
                    if (_incoming.Count == 0) break;
                    line = _incoming.Dequeue();
                }
                Handle(line);
            }
            if (_connected && unchecked(Environment.TickCount - _lastSync) >= (int)(SyncInterval * 1000))
            {
                Sync();
            }
        }

        public void Notify(Tone tone, Txt text)
        {
            var w = Message("notify");
            w.Key("tone").String(Control.ToneName(tone));
            w.Key("text");
            text.Write(w);
            Send(w);
        }

        /// <summary>Строка состояния мода в Modly. null — убрать.</summary>
        public void Status(Tone tone, Txt text)
        {
            var w = Message("status");
            w.Key("tone").String(Control.ToneName(tone));
            w.Key("text");
            (text ?? new Txt("")).Write(w);
            Send(w);
        }

        public void Log(string text)
        {
            var w = Message("log");
            w.Key("text").String(text);
            Send(w);
        }

        /// <summary>Отправить интерфейс заново (например, после перестройки страниц).</summary>
        public void Resend()
        {
            if (_connected) SendLayout();
        }

        public void Dispose()
        {
            if (_connected)
            {
                Send(Message("bye"));
                Thread.Sleep(40);
            }
            _stop = true;
        }

        // ------------------------------------------------------------------ сообщения

        internal static Json.Writer Message(string type)
        {
            var w = new Json.Writer();
            w.BeginObject();
            w.Key("type").String(type);
            return w;
        }

        private void Send(Json.Writer w)
        {
            w.EndObject();
            SendRaw(w.ToString());
        }

        private void SendRaw(string json)
        {
            if (!_connected) return;
            lock (_outgoing)
            {
                _outgoing.Enqueue(json + "\n");
            }
        }

        private void SendLayout()
        {
            SendRaw(BuildLayout(true));
            _lastSync = Environment.TickCount;
        }

        // Весь интерфейс одной строкой. markSent — запомнить отправленные значения (для рассылки изменений).
        private string BuildLayout(bool markSent)
        {
            var w = Message("layout");
            w.Key("pages").BeginArray();
            foreach (ModlyPage p in _pages)
            {
                w.BeginObject();
                w.Key("id").String(p.Id);
                w.Key("title");
                (p.Title ?? new Txt(p.Id)).Write(w);
                if (p.Icon != null) w.Key("icon").String(p.Icon);
                w.Key("sections").BeginArray();
                foreach (ModlySection s in p.Sections)
                {
                    w.BeginObject();
                    w.Key("id").String(s.Id);
                    if (s.Title != null) { w.Key("title"); s.Title.Write(w); }
                    if (s.DescriptionText != null) { w.Key("description"); s.DescriptionText.Write(w); }
                    w.Key("controls").BeginArray();
                    foreach (Control c in s.Controls)
                    {
                        Refresh(c);
                        string value = SafeRead(c);
                        if (markSent) c.SentValue = value;
                        c.WriteLayout(w, value);
                    }
                    w.EndArray();
                    w.EndObject();
                }
                w.EndArray();
                w.EndObject();
            }
            w.EndArray();
            w.EndObject();
            return w.ToString();
        }

        // пересчитать доступность, видимость и варианты; true — если что-то поменялось
        private bool Refresh(Control c)
        {
            bool changed = false;
            if (c.EnabledFunc != null)
            {
                bool e = SafeBool(c.EnabledFunc, c.Enabled);
                if (e != c.Enabled) { c.Enabled = e; changed = true; }
            }
            if (c.VisibleFunc != null)
            {
                bool v = SafeBool(c.VisibleFunc, c.Visible);
                if (v != c.Visible) { c.Visible = v; changed = true; }
            }
            if (c.OptionsFunc != null)
            {
                Option[] options = null;
                try { options = c.OptionsFunc(); } catch (Exception ex) { LogError(ex); }
                if (options != null)
                {
                    c.OptionList = options;
                    var w = new Json.Writer();
                    c.WriteOptions(w);
                    string sig = w.ToString();
                    if (sig != c.SentOptions) { c.SentOptions = sig; changed = true; }
                }
            }
            return changed;
        }

        private void Sync()
        {
            _lastSync = Environment.TickCount;
            Json.Writer values = null;
            Json.Writer patch = null;
            foreach (Control c in _controls.Values)
            {
                if (Refresh(c))
                {
                    if (patch == null)
                    {
                        patch = Message("patch");
                        patch.Key("controls").BeginObject();
                    }
                    patch.Key(c.Id).BeginObject();
                    patch.Key("enabled").Bool(c.Enabled);
                    patch.Key("visible").Bool(c.Visible);
                    if (c.OptionsFunc != null)
                    {
                        patch.Key("options");
                        c.WriteOptions(patch);
                    }
                    patch.EndObject();
                }
                string v = SafeRead(c);
                if (v != null && v != c.SentValue)
                {
                    c.SentValue = v;
                    if (values == null)
                    {
                        values = Message("state");
                        values.Key("values").BeginObject();
                    }
                    values.Key(c.Id).Raw(v);
                }
            }
            if (patch != null)
            {
                patch.EndObject();
                Send(patch);
            }
            if (values != null)
            {
                values.EndObject();
                Send(values);
            }
            SyncMaps();
        }

        // карты: целиком (новое подключение, сменилось описание) или только изменившиеся метки
        private void SyncMaps()
        {
            if (!_connected) return;
            foreach (ModlyMap m in _maps)
            {
                try
                {
                    if (m.Dirty)
                    {
                        m.Dirty = false;
                        SendRaw(m.Build());
                        continue;
                    }
                    string patch = m.BuildPatch();
                    if (patch != null) SendRaw(patch);
                }
                catch (Exception ex)
                {
                    LogError(ex);
                }
            }
        }

        private void HandleMapAction(Dictionary<string, object> msg)
        {
            long req = (long)Json.ToDouble(Get(msg, "req"));
            string mapId = Json.ToStr(Get(msg, "map"));
            string action = Json.ToStr(Get(msg, "action"));
            ModlyMap map = null;
            foreach (ModlyMap m in _maps)
            {
                if (m.Id == mapId) map = m;
            }
            Func<MapHit, string> handler;
            if (map == null || !map.Handlers.TryGetValue(action, out handler))
            {
                Reply(req, false, mapId + "/" + action + ": unknown");
                return;
            }
            object marker = Get(msg, "marker");
            var hit = new MapHit { Marker = marker == null ? null : Json.ToStr(marker), X = Json.ToDouble(Get(msg, "x")), Y = Json.ToDouble(Get(msg, "y")) };
            try
            {
                Reply(req, true, handler(hit));
            }
            catch (Exception ex)
            {
                Reply(req, false, ex.Message);
            }
            Sync();
        }

        internal void SendPatch(Control c, string field)
        {
            if (!_connected) return;
            var w = Message("patch");
            w.Key("controls").BeginObject();
            w.Key(c.Id).BeginObject();
            if (field == "label" && c.Label != null) { w.Key("label"); c.Label.Write(w); }
            w.EndObject();
            w.EndObject();
            Send(w);
        }

        private void Handle(string line)
        {
            Dictionary<string, object> msg;
            try
            {
                msg = Json.Parse(line) as Dictionary<string, object>;
            }
            catch (Exception)
            {
                return;
            }
            if (msg == null) return;
            string type = Json.ToStr(Get(msg, "type"));
            switch (type)
            {
                case "welcome":
                case "lang":
                    string lang = Json.ToStr(Get(msg, "lang"));
                    if (lang != Language)
                    {
                        Language = lang;
                        Action<string> handler = LanguageChanged;
                        if (handler != null)
                        {
                            try { handler(lang); } catch (Exception ex) { LogError(ex); }
                        }
                    }
                    break;
                case "set":
                    HandleSet(msg);
                    break;
                case "invoke":
                    HandleInvoke(msg);
                    break;
                case "map_action":
                    HandleMapAction(msg);
                    break;
                case "ping":
                    Send(Message("pong"));
                    break;
            }
        }

        private void HandleSet(Dictionary<string, object> msg)
        {
            long req = (long)Json.ToDouble(Get(msg, "req"));
            string id = Json.ToStr(Get(msg, "id"));
            Control c;
            if (!_controls.TryGetValue(id, out c) || c.Setter == null)
            {
                Reply(req, false, id + ": unknown");
                return;
            }
            try
            {
                c.Setter(Get(msg, "value"));
                Reply(req, true, null);
            }
            catch (Exception ex)
            {
                Reply(req, false, ex.Message);
            }
            Sync();
        }

        private void HandleInvoke(Dictionary<string, object> msg)
        {
            long req = (long)Json.ToDouble(Get(msg, "req"));
            string id = Json.ToStr(Get(msg, "id"));
            Control c;
            if (!_controls.TryGetValue(id, out c) || (c.Action == null && c.ActionWithText == null))
            {
                Reply(req, false, id + ": unknown");
                return;
            }
            try
            {
                string text = null;
                if (c.ActionWithText != null) text = c.ActionWithText();
                else c.Action();
                Reply(req, true, text);
            }
            catch (Exception ex)
            {
                Reply(req, false, ex.Message);
            }
            Sync();
        }

        private void Reply(long req, bool ok, string text)
        {
            var w = Message("result");
            w.Key("req").Number(req);
            w.Key("ok").Bool(ok);
            if (!string.IsNullOrEmpty(text)) w.Key("text").String(text);
            Send(w);
        }

        private static object Get(Dictionary<string, object> d, string key)
        {
            object v;
            return d.TryGetValue(key, out v) ? v : null;
        }

        private string SafeRead(Control c)
        {
            try
            {
                return c.ReadValue();
            }
            catch (Exception ex)
            {
                LogError(ex);
                return c.SentValue;
            }
        }

        private bool SafeBool(Func<bool> f, bool fallback)
        {
            try { return f(); } catch (Exception ex) { LogError(ex); return fallback; }
        }

        private void Safe(Action a)
        {
            try { a(); } catch (Exception ex) { LogError(ex); }
        }

        internal void LogError(Exception ex)
        {
            Log("error: " + ex.GetType().Name + ": " + ex.Message);
        }

        // ------------------------------------------------------------------ канал (фоновый поток)

        private void IoLoop()
        {
            var buffer = new byte[64 * 1024];
            var pending = new List<byte>();
            while (!_stop)
            {
                IntPtr pipe = Native.CreateFileW(Pipe ?? PipeName, Native.GENERIC_READ | Native.GENERIC_WRITE, 0, IntPtr.Zero, Native.OPEN_EXISTING, 0, IntPtr.Zero);
                if (pipe == Native.INVALID_HANDLE_VALUE)
                {
                    SleepUnlessStopped(2000);
                    continue;
                }
                try
                {
                    if (!Write(pipe, _helloLine)) continue;
                    string cached = _cachedLayout;
                    if (cached != null && !Write(pipe, cached + "\n")) continue;
                    lock (_outgoing) _outgoing.Clear();
                    pending.Clear();
                    _pausedSent = false;
                    _lastPump = Environment.TickCount;
                    _connected = true;
                    _generation++;
                    while (!_stop)
                    {
                        string[] lines = null;
                        lock (_outgoing)
                        {
                            if (_outgoing.Count > 0)
                            {
                                lines = _outgoing.ToArray();
                                _outgoing.Clear();
                            }
                        }
                        bool ok = true;
                        if (lines != null)
                        {
                            foreach (string l in lines)
                            {
                                if (!Write(pipe, l)) { ok = false; break; }
                            }
                        }
                        if (!ok) break;

                        // главный поток игры встал (пауза без фокуса) — сказать Modly, что команды подождут
                        bool stalled = unchecked(Environment.TickCount - _lastPump) > 1500;
                        if (stalled != _pausedSent)
                        {
                            _pausedSent = stalled;
                            if (!Write(pipe, "{\"type\":\"paused\",\"paused\":" + (stalled ? "true" : "false") + "}\n")) break;
                        }

                        uint avail;
                        if (!Native.PeekNamedPipe(pipe, IntPtr.Zero, 0, IntPtr.Zero, out avail, IntPtr.Zero)) break;
                        if (avail == 0)
                        {
                            Thread.Sleep(15);
                            continue;
                        }
                        uint read;
                        uint want = Math.Min(avail, (uint)buffer.Length);
                        if (!Native.ReadFile(pipe, buffer, want, out read, IntPtr.Zero) || read == 0) break;
                        for (int i = 0; i < read; i++)
                        {
                            if (buffer[i] == (byte)'\n')
                            {
                                string text = Encoding.UTF8.GetString(pending.ToArray());
                                pending.Clear();
                                if (text.Length > 0)
                                {
                                    lock (_incoming) _incoming.Enqueue(text);
                                }
                            }
                            else
                            {
                                pending.Add(buffer[i]);
                            }
                        }
                        if (pending.Count > 4 * 1024 * 1024) break;
                    }
                    if (_stop)
                    {
                        // «до свидания» из Dispose
                        string[] rest;
                        lock (_outgoing) { rest = _outgoing.ToArray(); _outgoing.Clear(); }
                        foreach (string l in rest) Write(pipe, l);
                    }
                }
                finally
                {
                    _connected = false;
                    Native.CloseHandle(pipe);
                }
            }
        }

        private static bool Write(IntPtr pipe, string text)
        {
            byte[] data = Encoding.UTF8.GetBytes(text);
            int offset = 0;
            while (offset < data.Length)
            {
                uint written;
                byte[] chunk = data;
                if (offset > 0)
                {
                    chunk = new byte[data.Length - offset];
                    Array.Copy(data, offset, chunk, 0, chunk.Length);
                }
                if (!Native.WriteFile(pipe, chunk, (uint)chunk.Length, out written, IntPtr.Zero) || written == 0) return false;
                offset += (int)written;
            }
            return true;
        }

        private void SleepUnlessStopped(int ms)
        {
            for (int t = 0; t < ms && !_stop; t += 50)
            {
                Thread.Sleep(50);
            }
        }

        private static class Native
        {
            public const uint GENERIC_READ = 0x80000000;
            public const uint GENERIC_WRITE = 0x40000000;
            public const uint OPEN_EXISTING = 3;
            public static readonly IntPtr INVALID_HANDLE_VALUE = new IntPtr(-1);

            [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
            public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);

            [DllImport("kernel32.dll", SetLastError = true)]
            public static extern bool PeekNamedPipe(IntPtr pipe, IntPtr buffer, uint size, IntPtr read, out uint available, IntPtr left);

            [DllImport("kernel32.dll", SetLastError = true)]
            public static extern bool ReadFile(IntPtr file, byte[] buffer, uint toRead, out uint read, IntPtr overlapped);

            [DllImport("kernel32.dll", SetLastError = true)]
            public static extern bool WriteFile(IntPtr file, byte[] buffer, uint toWrite, out uint written, IntPtr overlapped);

            [DllImport("kernel32.dll", SetLastError = true)]
            public static extern bool CloseHandle(IntPtr handle);

            [DllImport("kernel32.dll")]
            public static extern uint GetCurrentProcessId();
        }
    }

    /// <summary>Минимальный JSON: писатель и разборщик (без зависимостей, культура не влияет на числа).</summary>
    internal static class Json
    {
        public sealed class Writer
        {
            private readonly StringBuilder _sb = new StringBuilder(256);
            private bool _needComma;

            public Writer BeginObject() { Sep(); _sb.Append('{'); _needComma = false; return this; }
            public Writer EndObject() { _sb.Append('}'); _needComma = true; return this; }
            public Writer BeginArray() { Sep(); _sb.Append('['); _needComma = false; return this; }
            public Writer EndArray() { _sb.Append(']'); _needComma = true; return this; }

            public Writer Key(string key)
            {
                Sep();
                Quote(key);
                _sb.Append(':');
                _needComma = false;
                return this;
            }

            public Writer String(string s)
            {
                Sep();
                if (s == null) _sb.Append("null");
                else Quote(s);
                _needComma = true;
                return this;
            }

            public Writer Number(double d)
            {
                Sep();
                if (double.IsNaN(d) || double.IsInfinity(d)) _sb.Append('0');
                else if (d == Math.Floor(d) && Math.Abs(d) < 1e15) _sb.Append(((long)d).ToString(CultureInfo.InvariantCulture));
                else _sb.Append(d.ToString("R", CultureInfo.InvariantCulture));
                _needComma = true;
                return this;
            }

            public Writer Bool(bool b)
            {
                Sep();
                _sb.Append(b ? "true" : "false");
                _needComma = true;
                return this;
            }

            public Writer Raw(string json)
            {
                Sep();
                _sb.Append(json);
                _needComma = true;
                return this;
            }

            /// <summary>Значение любого простого типа: bool, числа, строки, массивы строк.</summary>
            public Writer Value(object v)
            {
                if (v == null) return Raw("null");
                if (v is bool) return Bool((bool)v);
                if (v is string) return String((string)v);
                if (v is int) return Number((int)v);
                if (v is long) return Number((long)v);
                if (v is float) return Number(Math.Round((float)v, 5));
                if (v is double) return Number((double)v);
                if (v is uint) return Number((uint)v);
                if (v is short) return Number((short)v);
                if (v is byte) return Number((byte)v);
                if (v is decimal) return Number((double)(decimal)v);
                var strings = v as IEnumerable<string>;
                if (strings != null)
                {
                    BeginArray();
                    foreach (string s in strings) String(s);
                    return EndArray();
                }
                if (v is Txt) { ((Txt)v).Write(this); return this; }
                return String(v.ToString());
            }

            private void Sep()
            {
                if (_needComma) _sb.Append(',');
            }

            private void Quote(string s)
            {
                _sb.Append('"');
                foreach (char c in s)
                {
                    switch (c)
                    {
                        case '"': _sb.Append("\\\""); break;
                        case '\\': _sb.Append("\\\\"); break;
                        case '\n': _sb.Append("\\n"); break;
                        case '\r': _sb.Append("\\r"); break;
                        case '\t': _sb.Append("\\t"); break;
                        default:
                            if (c < ' ') _sb.Append("\\u").Append(((int)c).ToString("x4"));
                            else _sb.Append(c);
                            break;
                    }
                }
                _sb.Append('"');
            }

            public override string ToString()
            {
                return _sb.ToString();
            }
        }

        /// <summary>Разобрать JSON: объекты — Dictionary&lt;string, object&gt;, массивы — List&lt;object&gt;, числа — double.</summary>
        public static object Parse(string text)
        {
            int i = 0;
            object v = ParseValue(text, ref i);
            return v;
        }

        public static bool ToBool(object v)
        {
            if (v is bool) return (bool)v;
            if (v is double) return (double)v != 0;
            var s = v as string;
            return s == "true" || s == "1";
        }

        public static double ToDouble(object v)
        {
            if (v is double) return (double)v;
            if (v is bool) return (bool)v ? 1 : 0;
            double d;
            var s = v as string;
            return s != null && double.TryParse(s, NumberStyles.Float, CultureInfo.InvariantCulture, out d) ? d : 0;
        }

        public static string ToStr(object v)
        {
            if (v == null) return null;
            var s = v as string;
            if (s != null) return s;
            if (v is double) return ((double)v).ToString(CultureInfo.InvariantCulture);
            if (v is bool) return (bool)v ? "true" : "false";
            return v.ToString();
        }

        public static string[] ToStrings(object v)
        {
            var list = v as List<object>;
            if (list == null) return new string[0];
            var result = new string[list.Count];
            for (int i = 0; i < list.Count; i++) result[i] = ToStr(list[i]);
            return result;
        }

        private static void SkipWs(string s, ref int i)
        {
            while (i < s.Length && (s[i] == ' ' || s[i] == '\t' || s[i] == '\n' || s[i] == '\r')) i++;
        }

        private static object ParseValue(string s, ref int i)
        {
            SkipWs(s, ref i);
            if (i >= s.Length) throw new FormatException("unexpected end");
            char c = s[i];
            if (c == '{')
            {
                var obj = new Dictionary<string, object>();
                i++;
                SkipWs(s, ref i);
                if (i < s.Length && s[i] == '}') { i++; return obj; }
                while (true)
                {
                    SkipWs(s, ref i);
                    string key = ParseString(s, ref i);
                    SkipWs(s, ref i);
                    if (i >= s.Length || s[i] != ':') throw new FormatException("expected :");
                    i++;
                    obj[key] = ParseValue(s, ref i);
                    SkipWs(s, ref i);
                    if (i < s.Length && s[i] == ',') { i++; continue; }
                    if (i < s.Length && s[i] == '}') { i++; return obj; }
                    throw new FormatException("expected , or }");
                }
            }
            if (c == '[')
            {
                var list = new List<object>();
                i++;
                SkipWs(s, ref i);
                if (i < s.Length && s[i] == ']') { i++; return list; }
                while (true)
                {
                    list.Add(ParseValue(s, ref i));
                    SkipWs(s, ref i);
                    if (i < s.Length && s[i] == ',') { i++; continue; }
                    if (i < s.Length && s[i] == ']') { i++; return list; }
                    throw new FormatException("expected , or ]");
                }
            }
            if (c == '"') return ParseString(s, ref i);
            if (s.Length - i >= 4 && string.CompareOrdinal(s, i, "true", 0, 4) == 0) { i += 4; return true; }
            if (s.Length - i >= 5 && string.CompareOrdinal(s, i, "false", 0, 5) == 0) { i += 5; return false; }
            if (s.Length - i >= 4 && string.CompareOrdinal(s, i, "null", 0, 4) == 0) { i += 4; return null; }
            int start = i;
            while (i < s.Length && "+-0123456789.eE".IndexOf(s[i]) >= 0) i++;
            if (start == i) throw new FormatException("unexpected " + c);
            return double.Parse(s.Substring(start, i - start), NumberStyles.Float, CultureInfo.InvariantCulture);
        }

        private static string ParseString(string s, ref int i)
        {
            if (s[i] != '"') throw new FormatException("expected string");
            i++;
            var sb = new StringBuilder();
            while (i < s.Length)
            {
                char c = s[i++];
                if (c == '"') return sb.ToString();
                if (c != '\\') { sb.Append(c); continue; }
                if (i >= s.Length) break;
                char e = s[i++];
                switch (e)
                {
                    case 'n': sb.Append('\n'); break;
                    case 'r': sb.Append('\r'); break;
                    case 't': sb.Append('\t'); break;
                    case 'b': sb.Append('\b'); break;
                    case 'f': sb.Append('\f'); break;
                    case 'u':
                        sb.Append((char)int.Parse(s.Substring(i, 4), NumberStyles.HexNumber, CultureInfo.InvariantCulture));
                        i += 4;
                        break;
                    default: sb.Append(e); break;
                }
            }
            throw new FormatException("unterminated string");
        }
    }
}
