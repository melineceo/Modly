using Modly;
using UnityEngine;
using UnityEngine.SceneManagement;

namespace ExampleMod
{
    // Точка входа. Modly (способ установки "unity-mono") прописывает этот метод в
    // RuntimeInitializeOnLoads.json, и Unity сама вызывает его после загрузки первой сцены —
    // BepInEx и другие загрузчики не нужны. Метод: public static void без параметров.
    public static class ModEntry
    {
        private static bool _started;

        public static void Init()
        {
            if (_started)
            {
                return;
            }
            _started = true;
            var host = new GameObject("ExampleMod");
            Object.DontDestroyOnLoad(host);
            host.AddComponent<ExampleBehaviour>();
            Debug.Log("[ExampleMod] loaded");
        }
    }

    public sealed class ExampleBehaviour : MonoBehaviour
    {
        private ModlyClient _modly;

        // Состояние мода. В настоящем моде здесь — то, что вы меняете в игре
        // (Time.timeScale, здоровье игрока, деньги…). Учтите: сборки игр урезаны, и чего-то из Unity API
        // в конкретной игре может не быть — компилятор сразу это покажет, мод собирается против её DLL.
        private bool _enabled = true;
        private float _multiplier = 1f;
        private int _count = 5;
        private string _mode = "normal";

        private void Awake()
        {
            _modly = new ModlyClient("example-mod", "Example Mod", "1.0.0")
            {
                Author = "you",
                GameId = "your-game",
                GameName = "Your Game",
            };

            ModlySection main = _modly.Page("main", Txt.Of("Главное", "Main"), "star")
                .Section("main", Txt.Of("Настройки мода", "Mod settings"));
            main.Toggle("enabled", Txt.Of("Мод включён", "Mod enabled"), () => _enabled, v =>
            {
                _enabled = v;
                Debug.Log("[ExampleMod] enabled = " + v);
            }).Icon("power");
            main.Slider("multiplier", Txt.Of("Множитель", "Multiplier"), 0.1f, 5f, () => _multiplier, v => _multiplier = v)
                .Step(0.1).Unit("×").Icon("gauge").EnabledWhen(() => _enabled);
            main.Number("count", Txt.Of("Сколько", "How many"), () => _count, v => _count = Mathf.Clamp(v, 1, 100))
                .Range(1, 100).Presets(1, 5, 10, 25).Icon("plus");
            main.Select("mode", Txt.Of("Режим", "Mode"), new[]
            {
                Option.Of("easy", Txt.Of("Лёгкий", "Easy"), "leaf"),
                Option.Of("normal", Txt.Of("Обычный", "Normal"), "star"),
                Option.Of("hard", Txt.Of("Сложный", "Hard"), "flame"),
            }, () => _mode, v => _mode = v);
            main.Button("hello", Txt.Of("Поздороваться", "Say hello"), () => "Hello × " + _count + " (" + _mode + ")")
                .Tone(Tone.Accent).Icon("sparkles");

            ModlySection info = _modly.Page("main", null).Section("info", Txt.Of("Сведения", "Info"));
            info.Stat("fps", "FPS", () => Mathf.RoundToInt(1f / Mathf.Max(Time.unscaledDeltaTime, 0.0001f))).Icon("gauge");
            info.Stat("scene", Txt.Of("Сцена", "Scene"), () => SceneManager.GetActiveScene().name).Icon("map-pin");
            info.Stat("uptime", Txt.Of("В игре", "Playing for"), () => Mathf.FloorToInt(Time.realtimeSinceStartup / 60f) + " min").Icon("clock");

            _modly.Start();
        }

        // Команды Modly выполняются здесь, на главном потоке Unity.
        private void Update()
        {
            _modly.Update();
        }

        private void OnApplicationQuit()
        {
            _modly.Dispose();
        }
    }
}
