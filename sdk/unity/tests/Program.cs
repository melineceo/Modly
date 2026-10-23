// Автотест C#-SDK Modly (../Modly.cs).
//
// Тест сам притворяется приложением Modly: поднимает сервер именованного канала, к которому подключается
// ModlyClient, и проверяет протокол со стороны мода. Главный поток теста — это «игра»: пока она идёт,
// примерно каждые 10 мс вызывается client.Update(), а на «паузе» — не вызывается.
// Сообщения мода разбираются System.Text.Json — независимо от JSON самого SDK.
//
// Запуск: dotnet run (или run.ps1). Код выхода 0 — всё прошло.

using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.IO.Pipes;
using System.Linq;
using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading;
using Modly;

namespace ModlyTests
{
    internal static class Program
    {
        // строка со всем, что JSON обязан экранировать, и с не-ASCII
        private const string Tricky = "Привет, \"мир\"\n\t\\ ✓ \u0001";

        private static int _passed;
        private static int _failed;
        private static int _pipes;
        private static Session _current;

        private static int Main()
        {
            Console.OutputEncoding = new UTF8Encoding(false);
            // в русской культуре дробная часть пишется через запятую — на JSON SDK это влиять не должно
            CultureInfo ru = CultureInfo.GetCultureInfo("ru-RU");
            CultureInfo.DefaultThreadCurrentCulture = ru;
            CultureInfo.CurrentCulture = ru;
            var watchdog = new Thread(() =>
            {
                Thread.Sleep(120000);
                Print(ConsoleColor.Red, "Тест висит дольше 2 минут — прерываю.");
                Environment.Exit(2);
            }) { IsBackground = true };
            watchdog.Start();

            Console.WriteLine("Автотест C#-SDK Modly (" + ModlyClient.SdkName + ")");
            Scenario("Мод подключается к Modly", MainScenario);
            Scenario("Modly запускается, когда игра стоит", LateModlyScenario);

            Console.WriteLine();
            if (_failed == 0)
            {
                Print(ConsoleColor.Green, "Все шаги пройдены (" + _passed + ").");
                return 0;
            }
            Print(ConsoleColor.Red, "Провалено шагов: " + _failed + ", пройдено: " + _passed + ".");
            return 1;
        }

        // ------------------------------------------------------------------ сценарии

        private static void MainScenario(Session s)
        {
            var mod = new DemoMod();
            string pipe = NewPipe();
            s.Modly = new FakeModly(pipe);
            s.Client = mod.Build(pipe);

            Step("hello — первым сообщением, с pid и sdk", () =>
            {
                s.Pumping = false; // до первого Update интерфейс ещё не собран
                s.Client.Start();
                s.Modly.Accept(3000);
                Eq(s.Next(), "", new
                {
                    type = "hello",
                    protocol = 1,
                    mod = new
                    {
                        id = "sdk-test",
                        name = "SDK Test",
                        version = "1.2.3",
                        author = "tester",
                        description = new { en = "Test mod", ru = "Тестовый мод" },
                        url = "https://example.com/mod",
                    },
                    game = new { id = "test-game", name = "Test Game", exe = "TestGame.exe" },
                    pid = Environment.ProcessId,
                    sdk = ModlyClient.SdkName,
                });
                Check(ModlyClient.SdkName.StartsWith("modly-unity/", StringComparison.Ordinal), "sdk: " + ModlyClient.SdkName);
            });

            Step("layout со всеми видами элементов — после первого Update", () =>
            {
                s.Pumping = true;
                JsonElement layout = s.Next();
                Eq(layout, "type", "layout");
                Eq(Skeleton(layout), "", ExpectedSkeleton());
                CheckControls(Controls(layout));
                Check(mod.Connects == 1, "событие ConnectedToModly: " + mod.Connects);
            });

            Step("карта — сразу после layout: края, категории, метки, живая точка, действия", () =>
            {
                Eq(s.Next(), "", new
                {
                    type = "map",
                    id = "world",
                    title = new { en = "World", ru = "Мир" },
                    image = @"C:\maps\world.png",
                    bounds = new { left = -100, top = 100, right = 100, bottom = -100 },
                    categories = new[] { new { id = "places", label = new { en = "Places", ru = "Места" }, icon = "map-pin", color = "#8b5cf6" } },
                    markers = new object[]
                    {
                        new { id = "home", x = 1, y = 2, cat = "places", title = "Home", actions = new[] { new { id = "tp", label = "Teleport" } } },
                        new { id = "player", x = 10, y = 20, title = "You", style = "dot" },
                    },
                    click_actions = new[] { new { id = "tp", label = "Teleport here", confirm = "Sure?" } },
                    focus = "player",
                });
            });

            Step("живая метка сдвинулась → map_patch только с ней", () =>
            {
                mod.PlayerX = 15.5;
                Eq(s.Wait("map_patch", m => Type(m) == "map_patch"), "", new
                {
                    type = "map_patch",
                    map = "world",
                    set = new[] { new { id = "player", x = 15.5, y = 20, title = "You", style = "dot" } },
                });
            });

            Step("map_action у метки и по клику в пустое место → обработчик и result с текстом", () =>
            {
                s.Modly.Send(new { type = "map_action", req = 40, map = "world", action = "tp", marker = "home", x = 1, y = 2 });
                Eq(s.Wait("result 40", m => IsResult(m, 40)), "", new { type = "result", req = 40, ok = true, text = "Teleported" });
                Check(mod.LastHit != null && mod.LastHit.Marker == "home", "у метки: " + (mod.LastHit == null ? "нет" : mod.LastHit.Marker));
                s.Modly.Send(new { type = "map_action", req = 41, map = "world", action = "tp", x = -50.5, y = 30 });
                Eq(s.Wait("result 41", m => IsResult(m, 41)), "", new { type = "result", req = 41, ok = true, text = "Teleported" });
                Check(mod.LastHit.Marker == null && mod.LastHit.X == -50.5 && mod.LastHit.Y == 30, "клик: " + mod.LastHit.X + ", " + mod.LastHit.Y);
                s.Modly.Send(new { type = "map_action", req = 42, map = "world", action = "nope", x = 0, y = 0 });
                Eq(s.Wait("result 42", m => IsResult(m, 42)), "ok", false);
            });

            Step("метку убрали → map_patch с remove", () =>
            {
                mod.Map.Remove("home");
                Eq(s.Wait("map_patch remove", m => Type(m) == "map_patch" && J.At(m, "remove").HasValue), "", new
                {
                    type = "map_patch",
                    map = "world",
                    remove = new[] { "home" },
                });
            });

            Step("welcome и lang → Language и LanguageChanged", () =>
            {
                s.Modly.Send(new { type = "welcome", protocol = 1, app = "Modly", version = "0.1.0", lang = "ru" });
                s.PumpUntil("Language = ru", () => s.Client.Language == "ru");
                s.Modly.Send(new { type = "lang", lang = "en" });
                s.PumpUntil("Language = en", () => s.Client.Language == "en");
                Check(string.Join(",", mod.Languages) == "ru,en", "LanguageChanged: " + string.Join(",", mod.Languages));
            });

            Step("ping → pong", () =>
            {
                s.Modly.Send(new { type = "ping" });
                Eq(s.Wait("pong", m => Type(m) == "pong"), "", new { type = "pong" });
            });

            Step("set → сеттер, result ok и state с новым значением", () =>
            {
                Set(s, 1, "god", true, true);
                Check(mod.God, "сеттер god не вызван");
                Set(s, 2, "speed", 99, 5); // сеттер ограничил значение — в state настоящее
                Check(mod.Speed == 5f, "speed = " + mod.Speed);
                Set(s, 3, "volume", 7.6, 8);
                Set(s, 4, "coins", 2500, 2500);
                Set(s, 5, "gravity", 1.62, 1.62);
                Check(mod.Gravity == 1.62, "gravity = " + mod.Gravity);
                Set(s, 6, "name", Tricky, Tricky);
                Check(mod.Name == Tricky, "name = " + mod.Name);
                Set(s, 7, "weather", "rain", "rain");
                Set(s, 8, "tags", new[] { "a", "c" }, new[] { "a", "c" });
                Check(string.Join(",", mod.Tags) == "a,c", "tags = " + string.Join(",", mod.Tags));
            });

            Step("set: строка в \\u-экранировании, с суррогатной парой", () =>
            {
                s.Modly.SendRaw(@"{""type"":""set"",""req"":9,""id"":""name"",""value"":""\u041f\u0440\u0438\u0432\u0435\u0442 \ud83d\ude00""}");
                Eq(s.Wait("result 9", m => IsResult(m, 9)), "", new { type = "result", req = 9, ok = true });
                Eq(s.Wait("state с name", m => HasValue(m, "name")), "values.name", "Привет 😀");
                Check(mod.Name == "Привет 😀", "name = " + mod.Name);
            });

            Step("set с ошибкой → result ok:false с текстом", () =>
            {
                string before = mod.Name;
                s.Modly.Send(new { type = "set", req = 10, id = "name", value = "" });
                Eq(s.Wait("result 10", m => IsResult(m, 10)), "", new { type = "result", req = 10, ok = false, text = "Имя не может быть пустым" });
                Check(mod.Name == before, "name изменилось: " + mod.Name);
                s.Modly.Send(new { type = "set", req = 11, id = "health", value = 1 }); // показатель: сеттера нет
                Eq(s.Wait("result 11", m => IsResult(m, 11)), "", new { type = "result", req = 11, ok = false, text = "health: unknown" });
                s.Modly.Send(new { type = "set", req = 12, id = "nope", value = 1 });
                Eq(s.Wait("result 12", m => IsResult(m, 12)), "", new { type = "result", req = 12, ok = false, text = "nope: unknown" });
            });

            Step("invoke → кнопка выполняется, result с её текстом", () =>
            {
                Eq(Invoke(s, 20, "heal"), "", new { type = "result", req = 20, ok = true, text = "Вылечено!" });
                Eq(Invoke(s, 21, "reset"), "", new { type = "result", req = 21, ok = true });
                Check(mod.Resets == 1, "reset вызван " + mod.Resets + " раз");
                Eq(Invoke(s, 22, "boom"), "", new { type = "result", req = 22, ok = false, text = "Сломалось" });
                Eq(Invoke(s, 23, "god"), "", new { type = "result", req = 23, ok = false, text = "god: unknown" });
            });

            Step("EnabledWhen / VisibleWhen / OptionsFrom → patch", () =>
            {
                s.Forget();
                mod.WorldLoaded = true;
                mod.ShowDebug = true;
                mod.Items = new[] { "log", "stone" };
                Eq(s.Wait("patch", m => Type(m) == "patch"), "", new
                {
                    type = "patch",
                    controls = new
                    {
                        summon = new { enabled = true, visible = true },
                        item = new
                        {
                            enabled = true,
                            visible = true,
                            options = new[] { new { value = "log", label = "log" }, new { value = "stone", label = "stone" } },
                        },
                        debug = new { enabled = true, visible = true },
                    },
                });
                mod.WorldLoaded = false;
                Eq(s.Wait("patch", m => Type(m) == "patch"), "", new { type = "patch", controls = new { summon = new { enabled = false, visible = true } } });
                mod.WorldLoaded = true;
                Eq(s.Wait("patch", m => Type(m) == "patch"), "", new { type = "patch", controls = new { summon = new { enabled = true, visible = true } } });
            });

            Step("кнопка с подписью: invoke → state с новой подписью", () =>
            {
                Eq(Invoke(s, 24, "summon"), "", new { type = "result", req = 24, ok = true });
                Check(mod.Summoned == 6, "summon вызван " + (mod.Summoned - 5) + " раз");
                Eq(s.Wait("state с summon", m => HasValue(m, "summon")), "", new { type = "state", values = new { summon = "Призвать 6" } });
            });

            Step("SetLabel → patch с подписью", () =>
            {
                mod.Summon.SetLabel(Txt.Of("Призыв", "Summon!"));
                Eq(s.Wait("patch", m => Type(m) == "patch"), "", new
                {
                    type = "patch",
                    controls = new { summon = new { label = new { en = "Summon!", ru = "Призыв" } } },
                });
            });

            Step("значения поменялись в игре → state; Notify, Status, Log", () =>
            {
                mod.Health = 50;
                mod.Loading = 0.75f;
                Eq(s.Wait("state", m => Type(m) == "state"), "", new { type = "state", values = new { health = 50, loading = 0.75 } });
                s.Client.Notify(Tone.Success, Txt.Of("Готово", "Done"));
                Eq(s.Wait("notify", m => Type(m) == "notify"), "", new { type = "notify", tone = "success", text = new { en = "Done", ru = "Готово" } });
                s.Client.Status(Tone.Warning, "Загрузите сохранение");
                Eq(s.Wait("status", m => Type(m) == "status"), "", new { type = "status", tone = "warning", text = "Загрузите сохранение" });
                s.Client.Log("строка в консоль");
                Eq(s.Wait("log", m => Type(m) == "log"), "", new { type = "log", text = "строка в консоль" });
            });

            Step("2 с без Update → paused:true, команды ждут; Update снова → paused:false", () =>
            {
                s.Forget();
                s.Pumping = false;
                var sw = Stopwatch.StartNew();
                s.Modly.Send(new { type = "set", req = 30, id = "god", value = false });
                Eq(s.Wait("paused:true", m => IsPaused(m, true)), "", new { type = "paused", paused = true });
                long at = sw.ElapsedMilliseconds;
                Check(at >= 1200, "paused пришёл слишком рано: через " + at + " мс");
                s.Idle(Math.Max(0, 2300 - (int)sw.ElapsedMilliseconds)); // добираем до 2 с с запасом
                Check(mod.God, "set выполнился без Update");
                Check(!s.Backlog.Any(m => Type(m) == "result"), "result пришёл без Update");
                s.Pumping = true;
                Eq(s.Wait("paused:false", m => IsPaused(m, false)), "", new { type = "paused", paused = false });
                Eq(s.Wait("result 30", m => IsResult(m, 30)), "", new { type = "result", req = 30, ok = true });
                Check(!mod.God, "set не выполнился после паузы");
            });

            Step("Dispose → bye и закрытие канала", () =>
            {
                s.Pumping = false;
                s.Client.Dispose();
                Eq(s.Wait("bye", m => Type(m) == "bye"), "", new { type = "bye" });
                s.WaitClosed(2000);
            });
        }

        private static void LateModlyScenario(Session s)
        {
            var mod = new DemoMod();
            string pipe = NewPipe();
            s.Client = mod.Build(pipe);

            Step("Modly ещё нет: мод работает и заранее собирает интерфейс", () =>
            {
                s.Client.Start();
                s.PumpFor(300);
                Check(!s.Client.Connected, "Connected без Modly");
                Check(mod.Connects == 0, "ConnectedToModly без Modly");
            });

            Step("игра стоит, Modly запустился → hello и layout из кэша сразу, без Update", () =>
            {
                s.Pumping = false;
                mod.God = true; // кэш собран раньше; настоящее значение придёт со свежим layout
                s.Modly = new FakeModly(pipe);
                s.Modly.Accept(5000); // SDK пробует подключиться раз в 2 с
                Eq(s.Next(), "type", "hello");
                JsonElement layout = s.Next();
                Eq(layout, "type", "layout");
                Eq(Skeleton(layout), "", ExpectedSkeleton());
                Check(mod.Connects == 0, "ConnectedToModly без Update");
            });

            Step("нет Update → paused:true; Update → paused:false, свежий layout, ConnectedToModly", () =>
            {
                Eq(s.Wait("paused:true", m => IsPaused(m, true)), "", new { type = "paused", paused = true });
                s.Pumping = true;
                JsonElement layout = s.Wait("layout", m => Type(m) == "layout");
                Eq(Controls(layout)["god"], "value", true);
                s.Wait("paused:false", m => IsPaused(m, false));
                Check(mod.Connects == 1, "событие ConnectedToModly: " + mod.Connects);
            });

            Step("Modly закрылся → DisconnectedFromModly; открылся снова → мод переподключается", () =>
            {
                s.Modly.Dispose();
                s.Modly = null;
                s.PumpUntil("DisconnectedFromModly", () => mod.Disconnects == 1);
                Check(!s.Client.Connected, "Connected после закрытия канала");
                s.Forget();
                s.Pumping = false;
                s.Modly = new FakeModly(pipe);
                s.Modly.Accept(5000);
                Eq(s.Next(), "type", "hello");
                Eq(s.Next(), "type", "layout");
                s.Pumping = true;
                s.Wait("layout", m => Type(m) == "layout");
                Check(mod.Connects == 2, "событие ConnectedToModly: " + mod.Connects);
            });
        }

        // ------------------------------------------------------------------ ожидаемый интерфейс

        // страницы и секции целиком, вместо элементов — их id по порядку
        private static object ExpectedSkeleton()
        {
            return new object[]
            {
                new
                {
                    id = "main",
                    title = new { en = "Main", ru = "Главное" },
                    icon = "star",
                    sections = new[]
                    {
                        new { id = "player", title = "Player", description = "Player cheats", controls = new[] { "god", "speed", "volume", "coins", "gravity", "name" } },
                    },
                },
                new
                {
                    id = "world",
                    title = "World",
                    sections = new[] { new { id = "world", controls = new[] { "weather", "tags", "item", "heal", "reset", "boom", "summon" } } },
                },
                new
                {
                    id = "info",
                    title = "Info",
                    icon = "info",
                    sections = new[]
                    {
                        new { id = "stats", title = new { en = "Stats", ru = "Показатели" }, controls = new[] { "health", "mood", "loading", "hint", "sep", "debug" } },
                    },
                },
            };
        }

        private static void CheckControls(Dictionary<string, JsonElement> c)
        {
            string kinds = string.Join(" ", c.Values.Select(x => Str(x, "kind")).Distinct().OrderBy(k => k, StringComparer.Ordinal));
            Check(kinds == "button note number progress select separator slider stat text toggle", "виды элементов: " + kinds);

            Ctl(c, new { id = "god", kind = "toggle", label = new { en = "God mode", ru = "Бессмертие" }, description = "No damage", icon = "shield", hotkey = "F1", value = false });
            Ctl(c, new { id = "speed", kind = "slider", label = "Speed", min = 0.5, max = 5, step = 0.1, unit = "×", value = 1.5 });
            Ctl(c, new { id = "volume", kind = "slider", label = "Volume", min = 0, max = 10, step = 1, value = 3 });
            Ctl(c, new { id = "coins", kind = "number", label = "Coins", min = 0, max = 99999, step = 1, presets = new[] { 100, 1000, 10000 }, value = 100 });
            Ctl(c, new { id = "gravity", kind = "number", label = "Gravity", step = 0.01, value = 9.81 });
            Ctl(c, new { id = "name", kind = "text", label = "Name", placeholder = "Имя", value = "Игрок" });
            Ctl(c, new
            {
                id = "weather",
                kind = "select",
                label = "Weather",
                options = new object[] { new { value = "clear", label = "Clear", icon = "sun" }, new { value = "rain", label = new { en = "Rain", ru = "Дождь" } } },
                value = "clear",
            });
            Ctl(c, new
            {
                id = "tags",
                kind = "select",
                label = "Tags",
                multiple = true,
                options = new[] { new { value = "a", label = "A" }, new { value = "b", label = "B" }, new { value = "c", label = "C" } },
                value = new[] { "a" },
            });
            Ctl(c, new { id = "item", kind = "select", label = "Item", options = new[] { new { value = "log", label = "log" } }, value = "log" });
            Ctl(c, new { id = "heal", kind = "button", label = "Heal", tone = "accent" });
            Ctl(c, new { id = "reset", kind = "button", label = "Reset", tone = "danger", confirm = "Точно?" });
            Ctl(c, new { id = "boom", kind = "button", label = "Boom" });
            Ctl(c, new { id = "summon", kind = "button", label = "Summon", enabled = false, value = "Призвать 5" });
            Ctl(c, new { id = "health", kind = "stat", label = "Health", unit = "HP", value = 87 });
            Ctl(c, new { id = "mood", kind = "stat", label = "Mood", value = "спокойно" });
            Ctl(c, new { id = "loading", kind = "progress", label = "Loading", value = 0.25 });
            Ctl(c, new { id = "hint", kind = "note", label = new { en = "Hint", ru = "Подсказка" }, tone = "info" });
            Ctl(c, new { id = "sep", kind = "separator" });
            Ctl(c, new { id = "debug", kind = "toggle", label = "Debug", visible = false, value = false });
        }

        // ------------------------------------------------------------------ команды Modly

        private static void Set(Session s, int req, string id, object value, object expected)
        {
            s.Modly.Send(new { type = "set", req = req, id = id, value = value });
            Eq(s.Wait("result " + req, m => IsResult(m, req)), "", new { type = "result", req = req, ok = true });
            // в state — только изменившееся значение
            Eq(s.Wait("state с " + id, m => HasValue(m, id)), "", new { type = "state", values = new Dictionary<string, object> { { id, expected } } });
        }

        private static JsonElement Invoke(Session s, int req, string id)
        {
            s.Modly.Send(new { type = "invoke", req = req, id = id });
            return s.Wait("result " + req, m => IsResult(m, req));
        }

        // ------------------------------------------------------------------ разбор сообщений

        private static string Type(JsonElement m)
        {
            return Str(m, "type");
        }

        private static bool IsResult(JsonElement m, int req)
        {
            JsonElement? r = J.At(m, "req");
            return Type(m) == "result" && r.HasValue && r.Value.ValueKind == JsonValueKind.Number && r.Value.GetDouble() == req;
        }

        private static bool IsPaused(JsonElement m, bool paused)
        {
            JsonElement? p = J.At(m, "paused");
            return Type(m) == "paused" && p.HasValue && p.Value.ValueKind == (paused ? JsonValueKind.True : JsonValueKind.False);
        }

        private static bool HasValue(JsonElement m, string id)
        {
            return Type(m) == "state" && J.At(m, "values." + id).HasValue;
        }

        private static string Str(JsonElement e, string path)
        {
            JsonElement? v = J.At(e, path);
            return v.HasValue && v.Value.ValueKind == JsonValueKind.String ? v.Value.GetString() : null;
        }

        private static Dictionary<string, JsonElement> Controls(JsonElement layout)
        {
            var result = new Dictionary<string, JsonElement>();
            foreach (JsonElement page in layout.GetProperty("pages").EnumerateArray())
            {
                foreach (JsonElement section in page.GetProperty("sections").EnumerateArray())
                {
                    foreach (JsonElement c in section.GetProperty("controls").EnumerateArray())
                    {
                        result[c.GetProperty("id").GetString()] = c;
                    }
                }
            }
            return result;
        }

        private static JsonElement Skeleton(JsonElement layout)
        {
            JsonNode pages = JsonNode.Parse(layout.GetRawText())["pages"];
            foreach (JsonNode page in pages.AsArray())
            {
                foreach (JsonNode section in page["sections"].AsArray())
                {
                    JsonNode[] ids = section["controls"].AsArray().Select(c => (JsonNode)JsonValue.Create(c["id"].GetValue<string>())).ToArray();
                    section["controls"] = new JsonArray(ids);
                }
            }
            return JsonSerializer.SerializeToElement(pages);
        }

        // ------------------------------------------------------------------ проверки

        private static void Check(bool condition, string what)
        {
            if (!condition) throw new TestFailed(what);
        }

        /// <summary>Значение по пути ("" — всё сообщение) совпадает с ожидаемым целиком, без лишних полей.</summary>
        private static void Eq(JsonElement e, string path, object expected)
        {
            JsonElement want = J.Of(expected);
            JsonElement? actual = J.At(e, path);
            string where = path.Length > 0 ? path : "сообщение";
            if (!actual.HasValue)
            {
                throw new TestFailed(where + ": нет поля в " + e.GetRawText());
            }
            if (!J.Same(actual.Value, want))
            {
                throw new TestFailed(where + ": " + actual.Value.GetRawText() + "\n         ожидалось: " + want.GetRawText());
            }
        }

        private static void Ctl(Dictionary<string, JsonElement> controls, object expected)
        {
            string id = J.Of(expected).GetProperty("id").GetString();
            JsonElement actual;
            Check(controls.TryGetValue(id, out actual), "в layout нет элемента " + id);
            Eq(actual, "", expected);
        }

        // ------------------------------------------------------------------ запуск

        private static string NewPipe()
        {
            return "modly-sdk-test-" + Environment.ProcessId + "-" + (++_pipes);
        }

        private static void Scenario(string title, Action<Session> body)
        {
            Console.WriteLine();
            Console.WriteLine(title);
            var s = new Session();
            _current = s;
            try
            {
                body(s);
            }
            catch (StepFailed)
            {
                Console.WriteLine("  (остальные шаги сценария пропущены)");
            }
            catch (Exception ex)
            {
                _failed++;
                Print(ConsoleColor.Red, "  [FAIL] подготовка сценария: " + ex);
            }
            finally
            {
                if (s.Client != null) s.Client.Dispose();
                if (s.Modly != null) s.Modly.Dispose();
            }
        }

        private static void Step(string name, Action body)
        {
            try
            {
                body();
            }
            catch (Exception ex)
            {
                _failed++;
                Print(ConsoleColor.Red, "  [FAIL] " + name);
                Console.WriteLine("         " + (ex is TestFailed ? ex.Message : ex.ToString()));
                FakeModly modly = _current.Modly;
                if (modly != null && modly.Received.Count > 0)
                {
                    Console.WriteLine("         последние сообщения мода:");
                    foreach (string line in modly.Received.Skip(Math.Max(0, modly.Received.Count - 8)))
                    {
                        Console.WriteLine("           " + (line.Length > 300 ? line.Substring(0, 300) + "…" : line));
                    }
                }
                throw new StepFailed();
            }
            _passed++;
            Print(ConsoleColor.Green, "  [ok] " + name);
        }

        private static void Print(ConsoleColor color, string text)
        {
            ConsoleColor old = Console.ForegroundColor;
            Console.ForegroundColor = color;
            Console.WriteLine(text);
            Console.ForegroundColor = old;
        }
    }

    /// <summary>«Мод»: его данные и интерфейс со всеми видами элементов.</summary>
    internal sealed class DemoMod
    {
        public bool God, DebugMode, WorldLoaded, ShowDebug;
        public float Speed = 1.5f, Loading = 0.25f;
        public int Volume = 3, Coins = 100, Summoned = 5, Health = 87, Resets;
        public double Gravity = 9.81;
        public string Name = "Игрок", Weather = "clear", Item = "log", Mood = "спокойно";
        public string[] Tags = { "a" };
        public string[] Items = { "log" };
        public Control Summon;
        public ModlyMap Map;
        public double PlayerX = 10, PlayerY = 20;
        public MapHit LastHit;
        public int Connects, Disconnects;
        public readonly List<string> Languages = new List<string>();

        public ModlyClient Build(string pipe)
        {
            var client = new ModlyClient("sdk-test", "SDK Test", "1.2.3")
            {
                Author = "tester",
                Description = Txt.Of("Тестовый мод", "Test mod"),
                Url = "https://example.com/mod",
                GameId = "test-game",
                GameName = "Test Game",
                GameExe = "TestGame.exe",
                Pipe = @"\\.\pipe\" + pipe,
                SyncInterval = 0.05f,
            };
            client.ConnectedToModly += () => Connects++;
            client.DisconnectedFromModly += () => Disconnects++;
            client.LanguageChanged += lang => Languages.Add(lang);

            ModlySection player = client.Page("main", Txt.Of("Главное", "Main"), "star").Section("player", "Player").Describe("Player cheats");
            player.Toggle("god", Txt.Of("Бессмертие", "God mode"), () => God, v => God = v).Icon("shield").Describe("No damage").Hotkey("F1");
            player.Slider("speed", "Speed", 0.5f, 5f, () => Speed, v => Speed = Math.Min(v, 5f)).Step(0.1).Unit("×");
            player.Slider("volume", "Volume", 0, 10, () => Volume, v => Volume = v);
            player.Number("coins", "Coins", () => Coins, v => Coins = v).Range(0, 99999).Presets(100, 1000, 10000);
            player.Number("gravity", "Gravity", () => Gravity, v => Gravity = v).Step(0.01);
            player.Text("name", "Name", () => Name, v =>
            {
                if (v.Length == 0) throw new InvalidOperationException("Имя не может быть пустым");
                Name = v;
            }).Placeholder("Имя");

            ModlySection world = client.Page("world", "World").Section("world");
            world.Select("weather", "Weather", new[] { Option.Of("clear", "Clear", "sun"), Option.Of("rain", Txt.Of("Дождь", "Rain")) }, () => Weather, v => Weather = v);
            world.MultiSelect("tags", "Tags", new[] { Option.Of("a", "A"), Option.Of("b", "B"), Option.Of("c", "C") }, () => Tags, v => Tags = v);
            world.Select("item", "Item", null, () => Item, v => Item = v).OptionsFrom(() => Items.Select(i => Option.Of(i, i)).ToArray());
            world.Button("heal", "Heal", () => "Вылечено!").Tone(Tone.Accent);
            world.Button("reset", "Reset", () => { Resets++; }).Tone(Tone.Danger).Confirm("Точно?");
            world.Button("boom", "Boom", (Action)(() => { throw new InvalidOperationException("Сломалось"); }));
            Summon = world.Button("summon", "Summon", () => "Призвать " + Summoned, () => Summoned++).EnabledWhen(() => WorldLoaded);

            ModlySection stats = client.Page("info", "Info", "info").Section("stats", Txt.Of("Показатели", "Stats"));
            stats.Stat("health", "Health", () => Health).Unit("HP");
            stats.Stat("mood", "Mood", () => Mood);
            stats.Progress("loading", "Loading", () => Loading);
            stats.Note("hint", Txt.Of("Подсказка", "Hint")).Tone(Tone.Info);
            stats.Separator("sep");
            stats.Toggle("debug", "Debug", () => DebugMode, v => DebugMode = v).VisibleWhen(() => ShowDebug);

            // карта: y мира растёт вверх, поэтому top больше bottom
            Map = client.Map("world", Txt.Of("Мир", "World"), @"C:\maps\world.png").Bounds(-100, 100, 100, -100);
            Map.Category("places", Txt.Of("Места", "Places")).Icon("map-pin").Color("#8b5cf6");
            Map.Marker("home", 1, 2).Cat("places").Title("Home").Action("tp", "Teleport");
            Map.Live("player", () => PlayerX, () => PlayerY).Dot().Title("You");
            Map.ClickAction("tp", "Teleport here").Confirm("Sure?");
            Map.On("tp", hit => { LastHit = hit; return "Teleported"; });
            Map.Focus("player");
            return client;
        }
    }

    /// <summary>Мод + поддельный Modly; главный поток теста — игровой цикл.</summary>
    internal sealed class Session
    {
        public ModlyClient Client;
        public FakeModly Modly;
        /// <summary>Идёт ли «игра»: пока да, все ожидания вызывают Client.Update() примерно раз в 10 мс.</summary>
        public bool Pumping = true;
        // пришедшие, но ещё не разобранные сообщения — Wait ищет сначала среди них
        private readonly List<JsonElement> _backlog = new List<JsonElement>();

        public IEnumerable<JsonElement> Backlog { get { return _backlog; } }

        public void Forget()
        {
            _backlog.Clear();
        }

        /// <summary>Строго следующее сообщение мода.</summary>
        public JsonElement Next(int timeoutMs = 3000)
        {
            if (_backlog.Count > 0)
            {
                JsonElement first = _backlog[0];
                _backlog.RemoveAt(0);
                return first;
            }
            var sw = Stopwatch.StartNew();
            while (sw.ElapsedMilliseconds < timeoutMs)
            {
                JsonElement? m = Receive(10);
                if (m.HasValue) return m.Value;
            }
            throw new TestFailed("мод ничего не прислал за " + timeoutMs + " мс");
        }

        /// <summary>Первое подходящее сообщение (из отложенных или новых); остальные откладываются.</summary>
        public JsonElement Wait(string what, Func<JsonElement, bool> match, int timeoutMs = 3000)
        {
            for (int i = 0; i < _backlog.Count; i++)
            {
                if (match(_backlog[i]))
                {
                    JsonElement found = _backlog[i];
                    _backlog.RemoveAt(i);
                    return found;
                }
            }
            var sw = Stopwatch.StartNew();
            while (sw.ElapsedMilliseconds < timeoutMs)
            {
                JsonElement? m = Receive(10);
                if (!m.HasValue) continue;
                if (match(m.Value)) return m.Value;
                _backlog.Add(m.Value);
            }
            throw new TestFailed("не дождались за " + timeoutMs + " мс: " + what);
        }

        public void PumpUntil(string what, Func<bool> done, int timeoutMs = 3000)
        {
            var sw = Stopwatch.StartNew();
            while (sw.ElapsedMilliseconds < timeoutMs)
            {
                JsonElement? m = Receive(10);
                if (m.HasValue) _backlog.Add(m.Value);
                if (done()) return;
            }
            throw new TestFailed("не дождались за " + timeoutMs + " мс: " + what);
        }

        /// <summary>Принимать сообщения ms миллисекунд (в отложенные).</summary>
        public void Idle(int ms)
        {
            var sw = Stopwatch.StartNew();
            while (sw.ElapsedMilliseconds < ms)
            {
                JsonElement? m = Receive(10);
                if (m.HasValue) _backlog.Add(m.Value);
            }
        }

        /// <summary>«Игра идёт» ms миллисекунд.</summary>
        public void PumpFor(int ms)
        {
            bool was = Pumping;
            Pumping = true;
            Idle(ms);
            Pumping = was;
        }

        public void WaitClosed(int timeoutMs)
        {
            var sw = Stopwatch.StartNew();
            while (sw.ElapsedMilliseconds < timeoutMs)
            {
                if (Pumping) Client.Update();
                string line = Modly.Take(10);
                if (line != null) _backlog.Add(Parse(line));
                if (Modly.Closed) return;
            }
            throw new TestFailed("мод не закрыл канал за " + timeoutMs + " мс");
        }

        // один «кадр»: Update, если игра идёт, и до waitMs ожидания строки от мода
        private JsonElement? Receive(int waitMs)
        {
            if (Pumping) Client.Update();
            if (Modly == null)
            {
                Thread.Sleep(waitMs);
                return null;
            }
            string line = Modly.Take(waitMs);
            if (line != null) return Parse(line);
            if (Modly.Closed) throw new TestFailed("мод закрыл канал");
            return null;
        }

        private static JsonElement Parse(string line)
        {
            try
            {
                return JsonSerializer.Deserialize<JsonElement>(line);
            }
            catch (JsonException ex)
            {
                throw new TestFailed("мод прислал не JSON: " + line + "\n         " + ex.Message);
            }
        }
    }

    /// <summary>Поддельный Modly: сервер канала; строки мода читаются в фоне.</summary>
    internal sealed class FakeModly : IDisposable
    {
        // как serde_json в Modly: не-ASCII — как есть, в UTF-8
        private static readonly JsonSerializerOptions SendOptions = new JsonSerializerOptions { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };

        private readonly NamedPipeServerStream _pipe;
        private readonly BlockingCollection<string> _lines = new BlockingCollection<string>(); // null — канал закрыт
        private StreamWriter _writer;

        /// <summary>Все строки, полученные от мода (для отчёта об ошибке).</summary>
        public readonly List<string> Received = new List<string>();
        public bool Closed { get; private set; }

        public FakeModly(string name)
        {
            // асинхронный канал: чтение в фоне не блокирует запись с главного потока
            _pipe = new NamedPipeServerStream(name, PipeDirection.InOut, NamedPipeServerStream.MaxAllowedServerInstances,
                PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
        }

        public void Accept(int timeoutMs)
        {
            if (!_pipe.WaitForConnectionAsync().Wait(timeoutMs))
            {
                throw new TestFailed("мод не подключился за " + timeoutMs + " мс");
            }
            _writer = new StreamWriter(_pipe, new UTF8Encoding(false)) { AutoFlush = true, NewLine = "\n" };
            var reader = new StreamReader(_pipe, new UTF8Encoding(false));
            var thread = new Thread(() =>
            {
                try
                {
                    string line;
                    while ((line = reader.ReadLine()) != null) _lines.Add(line);
                }
                catch (Exception)
                {
                    // канал закрыли
                }
                _lines.Add(null);
            }) { IsBackground = true, Name = "FakeModly" };
            thread.Start();
        }

        public void Send(object message)
        {
            SendRaw(JsonSerializer.Serialize(message, SendOptions));
        }

        public void SendRaw(string json)
        {
            _writer.Write(json + "\n");
        }

        /// <summary>Следующая строка от мода; null — за timeoutMs ничего не пришло или канал закрыт (см. Closed).</summary>
        public string Take(int timeoutMs)
        {
            if (Closed) return null;
            string line;
            if (!_lines.TryTake(out line, timeoutMs)) return null;
            if (line == null)
            {
                Closed = true;
                return null;
            }
            Received.Add(line);
            return line;
        }

        public void Dispose()
        {
            _pipe.Dispose();
        }
    }

    /// <summary>JSON-помощники поверх System.Text.Json.</summary>
    internal static class J
    {
        private static readonly JsonSerializerOptions Options = new JsonSerializerOptions { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };

        public static JsonElement Of(object value)
        {
            return JsonSerializer.SerializeToElement(value, Options);
        }

        /// <summary>Значение по пути "a.b.0.c" ("" — сам элемент); null — такого нет.</summary>
        public static JsonElement? At(JsonElement e, string path)
        {
            if (path.Length == 0) return e;
            foreach (string part in path.Split('.'))
            {
                int index;
                if (e.ValueKind == JsonValueKind.Object)
                {
                    if (!e.TryGetProperty(part, out e)) return null;
                }
                else if (e.ValueKind == JsonValueKind.Array && int.TryParse(part, out index) && index < e.GetArrayLength())
                {
                    e = e[index];
                }
                else
                {
                    return null;
                }
            }
            return e;
        }

        /// <summary>Равенство по смыслу: числа — по значению, у объектов — тот же набор полей.</summary>
        public static bool Same(JsonElement a, JsonElement b)
        {
            if (a.ValueKind != b.ValueKind) return false;
            switch (a.ValueKind)
            {
                case JsonValueKind.Number:
                    return a.GetDouble() == b.GetDouble();
                case JsonValueKind.String:
                    return a.GetString() == b.GetString();
                case JsonValueKind.Array:
                    return a.GetArrayLength() == b.GetArrayLength() && a.EnumerateArray().Zip(b.EnumerateArray(), Same).All(x => x);
                case JsonValueKind.Object:
                    List<JsonProperty> props = a.EnumerateObject().ToList();
                    return props.Count == b.EnumerateObject().Count()
                        && props.All(p => b.TryGetProperty(p.Name, out JsonElement other) && Same(p.Value, other));
                default:
                    return true; // true, false, null
            }
        }
    }

    internal sealed class TestFailed : Exception
    {
        public TestFailed(string message) : base(message) { }
    }

    // шаг уже напечатан как проваленный — остальные шаги сценария пропускаются
    internal sealed class StepFailed : Exception
    {
    }
}
