// Modly SDK для C++ — один заголовок, C++17, только WinAPI.
//
// Для модов Unreal Engine (UE4SS), инжектируемых DLL и своих движков. Опишите переключатели, ползунки
// и кнопки — Modly нарисует их и будет присылать изменения. Мод работает и без Modly.
//
//   #include "modly.hpp"
//   static bool god = false;
//   static float speed = 1.0f;
//
//   modly::Client modly({"my-mod", "My Mod", "1.0.0"});
//   auto& s = modly.page("main", "Main").section("player", "Player");
//   s.toggle("god", "God mode", &god).icon("shield");
//   s.slider("speed", "Speed", 0.5, 5.0, &speed).step(0.1);
//   s.button("heal", "Heal", [] { heal(); }).tone(modly::Tone::Accent);
//   modly.start();
//   // каждый кадр, на игровом потоке:
//   modly.update();
//
// Строки — UTF-8. Протокол — docs/PROTOCOL.md в репозитории Modly.

#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

namespace modly {

inline constexpr const wchar_t* kPipeName = L"\\\\.\\pipe\\modly";
inline constexpr const char* kSdkName = "modly-cpp/1.0.0";

enum class Tone { Default, Muted, Accent, Info, Success, Warning, Danger };

inline const char* tone_name(Tone t) {
    switch (t) {
        case Tone::Muted: return "muted";
        case Tone::Accent: return "accent";
        case Tone::Info: return "info";
        case Tone::Success: return "success";
        case Tone::Warning: return "warning";
        case Tone::Danger: return "danger";
        default: return "default";
    }
}

// =============================================================================================
// JSON

namespace json {

struct Value {
    enum class Type { Null, Bool, Number, String, Array, Object };
    Type type = Type::Null;
    bool b = false;
    double n = 0;
    std::string s;
    std::vector<Value> arr;
    std::vector<std::pair<std::string, Value>> obj;

    Value() = default;
    Value(bool v) : type(Type::Bool), b(v) {}
    Value(double v) : type(Type::Number), n(v) {}
    Value(int v) : type(Type::Number), n(v) {}
    Value(float v) : type(Type::Number), n(v) {}
    Value(const char* v) : type(Type::String), s(v) {}
    Value(std::string v) : type(Type::String), s(std::move(v)) {}

    const Value* get(const std::string& key) const {
        for (auto& kv : obj)
            if (kv.first == key) return &kv.second;
        return nullptr;
    }
    bool as_bool() const { return type == Type::Bool ? b : type == Type::Number ? n != 0 : false; }
    double as_number() const { return type == Type::Number ? n : type == Type::Bool ? (b ? 1 : 0) : std::atof(s.c_str()); }
    std::string as_string() const {
        if (type == Type::String) return s;
        if (type == Type::Number) {
            char buf[64];
            std::snprintf(buf, sizeof buf, "%.17g", n);
            return buf;
        }
        if (type == Type::Bool) return b ? "true" : "false";
        return {};
    }
    std::vector<std::string> as_strings() const {
        std::vector<std::string> out;
        for (auto& v : arr) out.push_back(v.as_string());
        return out;
    }
};

class Writer {
public:
    Writer& begin_object() { sep(); out_ += '{'; comma_ = false; return *this; }
    Writer& end_object() { out_ += '}'; comma_ = true; return *this; }
    Writer& begin_array() { sep(); out_ += '['; comma_ = false; return *this; }
    Writer& end_array() { out_ += ']'; comma_ = true; return *this; }
    Writer& key(const std::string& k) { sep(); quote(k); out_ += ':'; comma_ = false; return *this; }
    Writer& string(const std::string& v) { sep(); quote(v); comma_ = true; return *this; }
    Writer& boolean(bool v) { sep(); out_ += v ? "true" : "false"; comma_ = true; return *this; }
    Writer& raw(const std::string& v) { sep(); out_ += v; comma_ = true; return *this; }
    Writer& number(double v) {
        sep();
        char buf[64];
        if (!std::isfinite(v)) std::snprintf(buf, sizeof buf, "0");
        else if (v == std::floor(v) && std::fabs(v) < 1e15) std::snprintf(buf, sizeof buf, "%lld", static_cast<long long>(v));
        else std::snprintf(buf, sizeof buf, "%.10g", v);
        // запятая вместо точки в некоторых локалях
        for (char* p = buf; *p; ++p)
            if (*p == ',') *p = '.';
        out_ += buf;
        comma_ = true;
        return *this;
    }
    Writer& value(const Value& v) {
        switch (v.type) {
            case Value::Type::Null: return raw("null");
            case Value::Type::Bool: return boolean(v.b);
            case Value::Type::Number: return number(v.n);
            case Value::Type::String: return string(v.s);
            case Value::Type::Array:
                begin_array();
                for (auto& x : v.arr) value(x);
                return end_array();
            case Value::Type::Object:
                begin_object();
                for (auto& kv : v.obj) key(kv.first).value(kv.second);
                return end_object();
        }
        return *this;
    }
    const std::string& str() const { return out_; }

private:
    void sep() {
        if (comma_) out_ += ',';
    }
    void quote(const std::string& s) {
        out_ += '"';
        for (unsigned char c : s) {
            switch (c) {
                case '"': out_ += "\\\""; break;
                case '\\': out_ += "\\\\"; break;
                case '\n': out_ += "\\n"; break;
                case '\r': out_ += "\\r"; break;
                case '\t': out_ += "\\t"; break;
                default:
                    if (c < 0x20) {
                        char buf[8];
                        std::snprintf(buf, sizeof buf, "\\u%04x", c);
                        out_ += buf;
                    } else {
                        out_ += static_cast<char>(c);
                    }
            }
        }
        out_ += '"';
    }
    std::string out_;
    bool comma_ = false;
};

class Parser {
public:
    explicit Parser(const std::string& s) : s_(s) {}
    bool parse(Value& out) {
        ok_ = true;
        out = value();
        return ok_;
    }

private:
    void ws() {
        while (i_ < s_.size() && (s_[i_] == ' ' || s_[i_] == '\t' || s_[i_] == '\n' || s_[i_] == '\r')) ++i_;
    }
    Value fail() {
        ok_ = false;
        return {};
    }
    Value value() {
        ws();
        if (i_ >= s_.size()) return fail();
        char c = s_[i_];
        if (c == '{') {
            Value v;
            v.type = Value::Type::Object;
            ++i_;
            ws();
            if (i_ < s_.size() && s_[i_] == '}') { ++i_; return v; }
            while (ok_) {
                ws();
                std::string k = str();
                ws();
                if (i_ >= s_.size() || s_[i_] != ':') return fail();
                ++i_;
                v.obj.emplace_back(std::move(k), value());
                ws();
                if (i_ < s_.size() && s_[i_] == ',') { ++i_; continue; }
                if (i_ < s_.size() && s_[i_] == '}') { ++i_; return v; }
                return fail();
            }
            return fail();
        }
        if (c == '[') {
            Value v;
            v.type = Value::Type::Array;
            ++i_;
            ws();
            if (i_ < s_.size() && s_[i_] == ']') { ++i_; return v; }
            while (ok_) {
                v.arr.push_back(value());
                ws();
                if (i_ < s_.size() && s_[i_] == ',') { ++i_; continue; }
                if (i_ < s_.size() && s_[i_] == ']') { ++i_; return v; }
                return fail();
            }
            return fail();
        }
        if (c == '"') return Value(str());
        if (s_.compare(i_, 4, "true") == 0) { i_ += 4; return Value(true); }
        if (s_.compare(i_, 5, "false") == 0) { i_ += 5; return Value(false); }
        if (s_.compare(i_, 4, "null") == 0) { i_ += 4; return {}; }
        size_t start = i_;
        while (i_ < s_.size() && std::strchr("+-0123456789.eE", s_[i_])) ++i_;
        if (start == i_) return fail();
        return Value(std::strtod(s_.substr(start, i_ - start).c_str(), nullptr));
    }
    std::string str() {
        std::string out;
        if (i_ >= s_.size() || s_[i_] != '"') { ok_ = false; return out; }
        ++i_;
        while (i_ < s_.size()) {
            char c = s_[i_++];
            if (c == '"') return out;
            if (c != '\\') { out += c; continue; }
            if (i_ >= s_.size()) break;
            char e = s_[i_++];
            switch (e) {
                case 'n': out += '\n'; break;
                case 'r': out += '\r'; break;
                case 't': out += '\t'; break;
                case 'b': out += '\b'; break;
                case 'f': out += '\f'; break;
                case 'u': {
                    unsigned cp = static_cast<unsigned>(std::strtoul(s_.substr(i_, 4).c_str(), nullptr, 16));
                    i_ += 4;
                    // в UTF-8 (суррогатные пары игнорируем — в протоколе их не бывает)
                    if (cp < 0x80) out += static_cast<char>(cp);
                    else if (cp < 0x800) { out += static_cast<char>(0xC0 | (cp >> 6)); out += static_cast<char>(0x80 | (cp & 0x3F)); }
                    else { out += static_cast<char>(0xE0 | (cp >> 12)); out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F)); out += static_cast<char>(0x80 | (cp & 0x3F)); }
                    break;
                }
                default: out += e;
            }
        }
        ok_ = false;
        return out;
    }
    const std::string& s_;
    size_t i_ = 0;
    bool ok_ = true;
};

}  // namespace json

// =============================================================================================
// Текст и варианты

/// Текст с переводами: `modly::Text("Speed").ru("Скорость")`. Строка превращается в Text сама.
class Text {
public:
    Text() = default;
    Text(const char* s) : default_(s ? s : "") {}
    Text(std::string s) : default_(std::move(s)) {}
    Text& ru(std::string s) { return lang("ru", std::move(s)); }
    Text& lang(const std::string& code, std::string s) {
        tr_.emplace_back(code, std::move(s));
        return *this;
    }
    bool empty() const { return default_.empty() && tr_.empty(); }
    void write(json::Writer& w) const {
        if (tr_.empty()) {
            w.string(default_);
            return;
        }
        w.begin_object();
        w.key("en").string(default_);
        for (auto& kv : tr_) w.key(kv.first).string(kv.second);
        w.end_object();
    }

private:
    std::string default_;
    std::vector<std::pair<std::string, std::string>> tr_;
};

struct Option {
    std::string value;
    Text label;
    std::string icon;
    void write(json::Writer& w) const {
        w.begin_object();
        w.key("value").string(value);
        if (!label.empty()) { w.key("label"); label.write(w); }
        if (!icon.empty()) w.key("icon").string(icon);
        w.end_object();
    }
};

// =============================================================================================
// Элемент

class Control {
public:
    Control(std::string id, std::string kind, Text label) : id_(std::move(id)), kind_(std::move(kind)), label_(std::move(label)) {}

    Control& icon(std::string name) { icon_ = std::move(name); return *this; }
    Control& describe(Text t) { description_ = std::move(t); has_description_ = true; return *this; }
    Control& tone(Tone t) { tone_ = t; return *this; }
    /// Кнопка сработает только со второго нажатия.
    Control& confirm(Text t = Text()) { confirm_ = std::move(t); has_confirm_ = true; return *this; }
    Control& step(double v) { step_ = v; return *this; }
    Control& unit(std::string u) { unit_ = std::move(u); return *this; }
    Control& range(double lo, double hi) { min_ = lo; max_ = hi; return *this; }
    Control& presets(std::vector<double> v) { presets_ = std::move(v); return *this; }
    Control& placeholder(Text t) { placeholder_ = std::move(t); has_placeholder_ = true; return *this; }
    Control& hotkey(std::string k) { hotkey_ = std::move(k); return *this; }
    /// Доступность пересчитывается сама.
    Control& enabled_when(std::function<bool()> f) { enabled_fn_ = std::move(f); return *this; }
    Control& visible_when(std::function<bool()> f) { visible_fn_ = std::move(f); return *this; }
    /// Варианты выбора, которые меняются по ходу игры.
    Control& options_from(std::function<std::vector<Option>()> f) { options_fn_ = std::move(f); return *this; }

private:
    friend class Section;
    friend class Client;

    void write_layout(json::Writer& w, const std::string* value) const {
        w.begin_object();
        w.key("id").string(id_);
        w.key("kind").string(kind_);
        if (!label_.empty()) { w.key("label"); label_.write(w); }
        if (has_description_) { w.key("description"); description_.write(w); }
        if (!icon_.empty()) w.key("icon").string(icon_);
        if (!enabled_) w.key("enabled").boolean(false);
        if (!visible_) w.key("visible").boolean(false);
        if (!hotkey_.empty()) w.key("hotkey").string(hotkey_);
        if (tone_ != Tone::Default) w.key("tone").string(tone_name(tone_));
        if (has_confirm_) { w.key("confirm"); confirm_.write(w); }
        if (min_) w.key("min").number(*min_);
        if (max_) w.key("max").number(*max_);
        if (step_) w.key("step").number(*step_);
        if (!unit_.empty()) w.key("unit").string(unit_);
        if (has_placeholder_) { w.key("placeholder"); placeholder_.write(w); }
        if (!presets_.empty()) {
            w.key("presets").begin_array();
            for (double p : presets_) w.number(p);
            w.end_array();
        }
        if (kind_ == "select") {
            if (multiple_) w.key("multiple").boolean(true);
            w.key("options");
            write_options(w);
        }
        if (value) w.key("value").raw(*value);
        w.end_object();
    }

    void write_options(json::Writer& w) const {
        w.begin_array();
        for (auto& o : options_) o.write(w);
        w.end_array();
    }

    std::optional<std::string> read_value() const {
        if (!getter_) return std::nullopt;
        json::Writer w;
        w.value(getter_());
        return w.str();
    }

    std::string id_, kind_;
    Text label_, description_, confirm_, placeholder_;
    bool has_description_ = false, has_confirm_ = false, has_placeholder_ = false;
    std::string icon_, unit_, hotkey_;
    Tone tone_ = Tone::Default;
    std::optional<double> min_, max_, step_;
    std::vector<double> presets_;
    std::vector<Option> options_;
    bool multiple_ = false;
    std::function<json::Value()> getter_;
    std::function<void(const json::Value&)> setter_;
    std::function<std::string()> action_;
    std::function<bool()> enabled_fn_, visible_fn_;
    std::function<std::vector<Option>()> options_fn_;
    bool enabled_ = true, visible_ = true;
    std::optional<std::string> sent_value_;
    std::string sent_options_;
};

class Client;

class Section {
public:
    Section(Client& client, std::string id, Text title) : client_(client), id_(std::move(id)), title_(std::move(title)) {}

    Section& describe(Text t) { description_ = std::move(t); return *this; }

    Control& toggle(const std::string& id, Text label, bool* value) {
        return toggle(id, std::move(label), [value] { return *value; }, [value](bool v) { *value = v; });
    }
    Control& toggle(const std::string& id, Text label, std::function<bool()> get, std::function<void(bool)> set) {
        Control& c = add(id, "toggle", std::move(label));
        c.getter_ = [get] { return json::Value(get()); };
        c.setter_ = [set](const json::Value& v) { set(v.as_bool()); };
        return c;
    }

    Control& button(const std::string& id, Text label, std::function<void()> action) {
        Control& c = add(id, "button", std::move(label));
        c.action_ = [action] { action(); return std::string(); };
        return c;
    }
    /// Кнопка с итогом: вернула текст — Modly покажет его уведомлением.
    Control& button_with_result(const std::string& id, Text label, std::function<std::string()> action) {
        Control& c = add(id, "button", std::move(label));
        c.action_ = std::move(action);
        return c;
    }

    template <typename T>
    Control& slider(const std::string& id, Text label, double min, double max, T* value) {
        Control& c = add(id, "slider", std::move(label));
        c.min_ = min;
        c.max_ = max;
        c.getter_ = [value] { return json::Value(static_cast<double>(*value)); };
        c.setter_ = [value](const json::Value& v) { *value = static_cast<T>(round_if_int<T>(v.as_number())); };
        if constexpr (std::is_integral_v<T>) c.step_ = 1;
        return c;
    }

    template <typename T>
    Control& number(const std::string& id, Text label, T* value) {
        Control& c = add(id, "number", std::move(label));
        c.getter_ = [value] { return json::Value(static_cast<double>(*value)); };
        c.setter_ = [value](const json::Value& v) { *value = static_cast<T>(round_if_int<T>(v.as_number())); };
        if constexpr (std::is_integral_v<T>) c.step_ = 1;
        return c;
    }

    Control& text(const std::string& id, Text label, std::string* value) {
        Control& c = add(id, "text", std::move(label));
        c.getter_ = [value] { return json::Value(*value); };
        c.setter_ = [value](const json::Value& v) { *value = v.as_string(); };
        return c;
    }

    Control& select(const std::string& id, Text label, std::vector<Option> options, std::string* value) {
        Control& c = add(id, "select", std::move(label));
        c.options_ = std::move(options);
        c.getter_ = [value] { return json::Value(*value); };
        c.setter_ = [value](const json::Value& v) { *value = v.as_string(); };
        return c;
    }

    Control& multi_select(const std::string& id, Text label, std::vector<Option> options, std::vector<std::string>* value) {
        Control& c = add(id, "select", std::move(label));
        c.options_ = std::move(options);
        c.multiple_ = true;
        c.getter_ = [value] {
            json::Value v;
            v.type = json::Value::Type::Array;
            for (auto& s : *value) v.arr.emplace_back(s);
            return v;
        };
        c.setter_ = [value](const json::Value& v) { *value = v.as_strings(); };
        return c;
    }

    /// Показатель только для чтения.
    Control& stat(const std::string& id, Text label, std::function<json::Value()> get) {
        Control& c = add(id, "stat", std::move(label));
        c.getter_ = std::move(get);
        return c;
    }

    /// Полоса прогресса, 0..1.
    Control& progress(const std::string& id, Text label, std::function<double()> get) {
        Control& c = add(id, "progress", std::move(label));
        c.getter_ = [get] { return json::Value(get()); };
        return c;
    }

    Control& note(const std::string& id, Text text) { return add(id, "note", std::move(text)); }
    Control& separator(const std::string& id) { return add(id, "separator", Text()); }

private:
    friend class Client;
    friend class Page;
    template <typename T>
    static double round_if_int(double v) {
        if constexpr (std::is_integral_v<T>) {
            return std::round(v);
        } else {
            return v;
        }
    }
    Control& add(const std::string& id, const char* kind, Text label);

    Client& client_;
    std::string id_;
    Text title_, description_;
    std::vector<std::unique_ptr<Control>> controls_;
};

class Page {
public:
    Page(Client& client, std::string id, Text title, std::string icon)
        : client_(client), id_(std::move(id)), title_(std::move(title)), icon_(std::move(icon)) {}

    Section& section(const std::string& id, Text title = Text()) {
        for (auto& s : sections_)
            if (s->id_ == id) return *s;
        sections_.push_back(std::make_unique<Section>(client_, id, std::move(title)));
        return *sections_.back();
    }

private:
    friend class Client;
    Client& client_;
    std::string id_;
    Text title_;
    std::string icon_;
    std::vector<std::unique_ptr<Section>> sections_;
};

// =============================================================================================
// Карты

/// Где сработало действие на карте: у метки (`marker` — её id) или по клику в пустое место
/// (`marker` пустой). `x`, `y` — в координатах игры (тех же, что края карты и метки).
struct MapHit {
    std::string marker;
    double x = 0, y = 0;
};

/// Действие на карте: кнопка в подписи метки («Телепорт») или по клику в пустое место.
class MapAction {
public:
    MapAction(std::string id, Text label) : id_(std::move(id)), label_(std::move(label)) {}
    MapAction& tone(Tone t) { tone_ = t; return *this; }
    /// Сработает только со второго нажатия.
    MapAction& confirm(Text t = Text()) { confirm_ = std::move(t); has_confirm_ = true; return *this; }
    MapAction& icon(std::string name) { icon_ = std::move(name); return *this; }

    void write(json::Writer& w) const {
        w.begin_object().key("id").string(id_);
        if (!label_.empty()) { w.key("label"); label_.write(w); }
        if (tone_ != Tone::Default) w.key("tone").string(tone_name(tone_));
        if (has_confirm_) { w.key("confirm"); confirm_.write(w); }
        if (!icon_.empty()) w.key("icon").string(icon_);
        w.end_object();
    }

private:
    friend class Map;
    std::string id_;
    Text label_, confirm_;
    bool has_confirm_ = false;
    Tone tone_ = Tone::Default;
    std::string icon_;
};

/// Категория меток: её можно скрыть на карте.
class MapCategory {
public:
    MapCategory(std::string id, Text label, bool* dirty) : id_(std::move(id)), label_(std::move(label)), dirty_(dirty) {}
    MapCategory& icon(std::string name) { icon_ = std::move(name); *dirty_ = true; return *this; }
    /// Цвет `"#rrggbb"`.
    MapCategory& color(std::string hex) { color_ = std::move(hex); *dirty_ = true; return *this; }
    /// Скрыта, пока игрок её не включит.
    MapCategory& hidden() { hidden_ = true; *dirty_ = true; return *this; }

    void write(json::Writer& w) const {
        w.begin_object().key("id").string(id_);
        if (!label_.empty()) { w.key("label"); label_.write(w); }
        if (!icon_.empty()) w.key("icon").string(icon_);
        if (!color_.empty()) w.key("color").string(color_);
        if (hidden_) w.key("hidden").boolean(true);
        w.end_object();
    }

private:
    friend class Map;
    std::string id_;
    Text label_;
    std::string icon_, color_;
    bool hidden_ = false;
    bool* dirty_;
};

/// Метка на карте: `.cat("places").title("Дом").action("tp", "Телепорт")`. Изменения после отправки
/// SDK сам присылает Modly.
class MapMarker {
public:
    MapMarker(std::string id, double x, double y) : id_(std::move(id)), x_(x), y_(y) {}
    MapMarker& cat(std::string id) { cat_ = std::move(id); return touch(); }
    MapMarker& title(Text t) { title_ = std::move(t); return touch(); }
    MapMarker& text(Text t) { text_ = std::move(t); has_text_ = true; return touch(); }
    MapMarker& icon(std::string name) { icon_ = std::move(name); return touch(); }
    /// Цвет `"#rrggbb"` (иначе — цвет категории).
    MapMarker& color(std::string hex) { color_ = std::move(hex); return touch(); }
    /// Точка вместо булавки — для того, что движется: игрок, питомец.
    MapMarker& dot() { dot_ = true; return touch(); }
    /// Кнопка в подписи метки; что делать — `Map::on`.
    MapMarker& action(std::string id, Text label) { actions_.emplace_back(std::move(id), std::move(label)); return touch(); }
    MapMarker& action(MapAction a) { actions_.push_back(std::move(a)); return touch(); }
    /// Передвинуть метку.
    MapMarker& move_to(double x, double y) { x_ = x; y_ = y; return touch(); }

private:
    friend class Map;
    MapMarker& touch() { changed_ = true; return *this; }
    bool position(double& x, double& y) const {
        x = x_;
        y = y_;
        if (!live_) return true;
        try {
            auto p = live_();
            x = p.first;
            y = p.second;
            return true;
        } catch (...) {
            return false;
        }
    }
    void write(json::Writer& w, double x, double y) const {
        w.begin_object().key("id").string(id_).key("x").number(x).key("y").number(y);
        if (!cat_.empty()) w.key("cat").string(cat_);
        if (!title_.empty()) { w.key("title"); title_.write(w); }
        if (has_text_) { w.key("text"); text_.write(w); }
        if (!icon_.empty()) w.key("icon").string(icon_);
        if (!color_.empty()) w.key("color").string(color_);
        if (dot_) w.key("style").string("dot");
        if (!actions_.empty()) {
            w.key("actions").begin_array();
            for (auto& a : actions_) a.write(w);
            w.end_array();
        }
        w.end_object();
    }

    std::string id_;
    double x_, y_;
    std::string cat_, icon_, color_;
    Text title_, text_;
    bool has_text_ = false, dot_ = false;
    std::vector<MapAction> actions_;
    std::function<std::pair<double, double>()> live_;
    bool changed_ = true, sent_ = false;
    double sent_x_ = 0, sent_y_ = 0;
};

/// Карта мода: картинка игры, её края в координатах игры, категории и метки. Создаётся `Client::map`.
class Map {
public:
    Map(std::string id, Text title, std::string image) : id_(std::move(id)), title_(std::move(title)), image_(std::move(image)) {}

    /// Края картинки в координатах игры. Без них координаты меток — пиксели картинки.
    /// Если y игры растёт вверх, `top` больше `bottom`.
    Map& bounds(double left, double top, double right, double bottom) {
        has_bounds_ = true;
        left_ = left;
        top_ = top;
        right_ = right;
        bottom_ = bottom;
        dirty_ = true;
        return *this;
    }
    MapCategory& category(const std::string& id, Text label) {
        erase_if(categories_, [&](auto& c) { return c->id_ == id; });
        categories_.push_back(std::make_unique<MapCategory>(id, std::move(label), &dirty_));
        dirty_ = true;
        return *categories_.back();
    }
    /// Метка (тот же id заменяет прежнюю).
    MapMarker& marker(const std::string& id, double x, double y) {
        erase_if(markers_, [&](auto& m) { return m->id_ == id; });
        removed_.erase(std::remove(removed_.begin(), removed_.end(), id), removed_.end());
        markers_.push_back(std::make_unique<MapMarker>(id, x, y));
        return *markers_.back();
    }
    /// Живая метка: место берётся из `pos()` и само присылается Modly, когда меняется.
    MapMarker& live(const std::string& id, std::function<std::pair<double, double>()> pos) {
        MapMarker& m = marker(id, 0, 0);
        m.live_ = std::move(pos);
        return m;
    }
    /// Живая метка по указателям на координаты (например, на поля игрока).
    template <typename T>
    MapMarker& live(const std::string& id, const T* x, const T* y) {
        return live(id, [x, y] { return std::make_pair(static_cast<double>(*x), static_cast<double>(*y)); });
    }
    /// Действие по клику в пустое место карты («Телепорт сюда»).
    MapAction& click_action(const std::string& id, Text label) {
        erase_if(click_, [&](auto& a) { return a->id_ == id; });
        click_.push_back(std::make_unique<MapAction>(id, std::move(label)));
        dirty_ = true;
        return *click_.back();
    }
    /// Что делать по действию: у любой метки с этим действием и по клику в пустое место.
    /// Вернули текст — Modly покажет его уведомлением; исключение — ошибкой.
    Map& on(const std::string& action, std::function<std::string(const MapHit&)> handler) {
        handlers_[action] = std::move(handler);
        return *this;
    }
    /// Метка, к которой карта подъезжает кнопкой «Где я» (обычно игрок).
    Map& focus(std::string marker_id) {
        focus_ = std::move(marker_id);
        dirty_ = true;
        return *this;
    }
    /// Убрать метку.
    void remove(const std::string& id) {
        size_t before = markers_.size();
        erase_if(markers_, [&](auto& m) { return m->id_ == id; });
        if (markers_.size() != before && std::find(removed_.begin(), removed_.end(), id) == removed_.end()) removed_.push_back(id);
    }
    MapMarker* find(const std::string& id) {
        for (auto& m : markers_)
            if (m->id_ == id) return m.get();
        return nullptr;
    }

private:
    friend class Client;
    template <typename V, typename P>
    static void erase_if(V& v, P pred) {
        v.erase(std::remove_if(v.begin(), v.end(), pred), v.end());
    }

    std::string build() {
        json::Writer w;
        w.begin_object().key("type").string("map").key("id").string(id_);
        if (!title_.empty()) { w.key("title"); title_.write(w); }
        w.key("image").string(image_);
        if (has_bounds_) {
            w.key("bounds").begin_object().key("left").number(left_).key("top").number(top_).key("right").number(right_).key("bottom").number(bottom_).end_object();
        }
        w.key("categories").begin_array();
        for (auto& c : categories_) c->write(w);
        w.end_array().key("markers").begin_array();
        for (auto& m : markers_) {
            double x, y;
            if (!m->position(x, y)) continue;
            m->sent_x_ = x;
            m->sent_y_ = y;
            m->sent_ = true;
            m->changed_ = false;
            m->write(w, x, y);
        }
        w.end_array();
        if (!click_.empty()) {
            w.key("click_actions").begin_array();
            for (auto& a : click_) a->write(w);
            w.end_array();
        }
        if (!focus_.empty()) w.key("focus").string(focus_);
        w.end_object();
        removed_.clear();
        return w.str();
    }

    // изменения с прошлого раза (метки и живые точки); пусто — нечего слать
    std::string build_patch() {
        json::Writer w;
        bool any = false;
        w.begin_object().key("type").string("map_patch").key("map").string(id_);
        for (auto& m : markers_) {
            double x, y;
            if (!m->position(x, y)) continue;
            if (!m->changed_ && m->sent_ && x == m->sent_x_ && y == m->sent_y_) continue;
            if (!any) w.key("set").begin_array();
            any = true;
            m->sent_x_ = x;
            m->sent_y_ = y;
            m->sent_ = true;
            m->changed_ = false;
            m->write(w, x, y);
        }
        if (any) w.end_array();
        if (!removed_.empty()) {
            w.key("remove").begin_array();
            for (auto& id : removed_) w.string(id);
            w.end_array();
        }
        if (!any && removed_.empty()) return {};
        removed_.clear();
        w.end_object();
        return w.str();
    }

    std::string id_;
    Text title_;
    std::string image_;
    bool has_bounds_ = false;
    double left_ = 0, top_ = 0, right_ = 0, bottom_ = 0;
    std::vector<std::unique_ptr<MapCategory>> categories_;
    std::vector<std::unique_ptr<MapMarker>> markers_;
    std::vector<std::unique_ptr<MapAction>> click_;
    std::map<std::string, std::function<std::string(const MapHit&)>> handlers_;
    std::string focus_;
    std::vector<std::string> removed_;
    bool dirty_ = true;
};

// =============================================================================================
// Клиент

struct ModInfo {
    std::string id;
    std::string name;
    std::string version;
    std::string author;
    std::string game_id;
    std::string game_name;
    std::string game_exe;
};

class Client {
public:
    explicit Client(ModInfo info) : info_(std::move(info)) {}
    ~Client() { stop(); }
    Client(const Client&) = delete;
    Client& operator=(const Client&) = delete;

    /// Канал (для тестов можно подменить до start()).
    std::wstring pipe_name = kPipeName;
    /// Как часто рассылать изменившиеся значения.
    std::chrono::milliseconds sync_interval{200};
    /// Подключились к Modly (интерфейс уже отправлен) / связь пропала.
    std::function<void()> on_connected, on_disconnected;
    /// Язык Modly сменился ("ru", "en"…).
    std::function<void(const std::string&)> on_language;

    Page& page(const std::string& id, Text title, std::string icon = {}) {
        for (auto& p : pages_)
            if (p->id_ == id) return *p;
        pages_.push_back(std::make_unique<Page>(*this, id, std::move(title), std::move(icon)));
        return *pages_.back();
    }

    bool connected() const { return connected_.load(); }
    std::string language() const { return language_; }

    /// Карта мода. `image` — полный путь к PNG, JPEG или WebP на этом компьютере (`C:\…\map.png`, UTF-8)
    /// или `data_url(...)`. Тот же id даёт ту же карту.
    Map& map(const std::string& id, Text title, std::string image) {
        for (auto& m : maps_) {
            if (m->id_ == id) {
                m->title_ = std::move(title);
                m->image_ = std::move(image);
                m->dirty_ = true;
                return *m;
            }
        }
        maps_.push_back(std::make_unique<Map>(id, std::move(title), std::move(image)));
        return *maps_.back();
    }

    /// Убрать карту целиком.
    void remove_map(const std::string& id) {
        size_t before = maps_.size();
        maps_.erase(std::remove_if(maps_.begin(), maps_.end(), [&](auto& m) { return m->id_ == id; }), maps_.end());
        if (maps_.size() != before) {
            json::Writer w;
            w.begin_object().key("type").string("map_remove").key("map").string(id).end_object();
            push(w.str());
        }
    }

    /// Картинка, вшитая в мод: `data:`-адрес для `map()` (до ~3 МБ — предел сообщения 4 МиБ).
    static std::string data_url(const std::string& mime, const void* data, size_t size) {
        static const char* t = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        auto* p = static_cast<const unsigned char*>(data);
        std::string out = "data:" + mime + ";base64,";
        out.reserve(out.size() + (size + 2) / 3 * 4);
        for (size_t i = 0; i < size; i += 3) {
            unsigned n = p[i] << 16 | (i + 1 < size ? p[i + 1] : 0) << 8 | (i + 2 < size ? p[i + 2] : 0);
            out += t[n >> 18 & 63];
            out += t[n >> 12 & 63];
            out += i + 1 < size ? t[n >> 6 & 63] : '=';
            out += i + 2 < size ? t[n & 63] : '=';
        }
        return out;
    }

    /// Начать ждать Modly в фоне.
    void start() {
        if (thread_.joinable()) return;
        json::Writer w;
        w.begin_object();
        w.key("type").string("hello");
        w.key("protocol").number(1);
        w.key("mod").begin_object();
        w.key("id").string(info_.id);
        w.key("name").string(info_.name);
        w.key("version").string(info_.version);
        if (!info_.author.empty()) w.key("author").string(info_.author);
        w.end_object();
        if (!info_.game_id.empty() || !info_.game_name.empty()) {
            w.key("game").begin_object();
            w.key("id").string(info_.game_id);
            w.key("name").string(info_.game_name);
            if (!info_.game_exe.empty()) w.key("exe").string(info_.game_exe);
            w.end_object();
        }
        w.key("pid").number(static_cast<double>(GetCurrentProcessId()));
        w.key("sdk").string(kSdkName);
        w.end_object();
        hello_ = w.str() + "\n";
        stop_ = false;
        thread_ = std::thread([this] { io_loop(); });
    }

    void stop() {
        if (!thread_.joinable()) return;
        if (connected_) push(R"({"type":"bye"})");
        std::this_thread::sleep_for(std::chrono::milliseconds(30));
        stop_ = true;
        thread_.join();
    }

    /// Каждый кадр на игровом потоке: команды Modly и рассылка изменившихся значений.
    void update() {
        if (!thread_.joinable()) return;
        last_pump_.store(now_ms());
        if (was_connected_ && !connected_) {
            was_connected_ = false;
            if (on_disconnected) on_disconnected();
        }
        if (connected_ && generation_.load() != sent_generation_) {
            sent_generation_ = generation_.load();
            push(build_layout(true));
            for (auto& m : maps_) m->dirty_ = true;
            sync_maps();
            last_sync_ = now_ms();
            was_connected_ = true;
            if (on_connected) on_connected();
        }
        if (!connected_ && (cached_layout_time_ == 0 || now_ms() - cached_layout_time_ > 2000)) {
            cached_layout_time_ = now_ms();
            std::lock_guard<std::mutex> lock(cache_mutex_);
            cached_layout_ = build_layout(false);
        }
        std::deque<std::string> lines;
        {
            std::lock_guard<std::mutex> lock(in_mutex_);
            lines.swap(incoming_);
        }
        for (auto& l : lines) handle(l);
        if (connected_ && now_ms() - last_sync_ >= sync_interval.count()) sync();
    }

    void notify(Tone tone, const Text& text) {
        json::Writer w;
        w.begin_object().key("type").string("notify").key("tone").string(tone_name(tone)).key("text");
        text.write(w);
        w.end_object();
        push(w.str());
    }

    /// Строка состояния мода. Пустой текст — убрать.
    void status(Tone tone, const Text& text) {
        json::Writer w;
        w.begin_object().key("type").string("status").key("tone").string(tone_name(tone)).key("text");
        text.write(w);
        w.end_object();
        push(w.str());
    }

    void log(const std::string& text) {
        json::Writer w;
        w.begin_object().key("type").string("log").key("text").string(text).end_object();
        push(w.str());
    }

private:
    friend class Section;

    static long long now_ms() {
        using namespace std::chrono;
        return duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count();
    }

    void push(const std::string& json) {
        if (!connected_) return;
        std::lock_guard<std::mutex> lock(out_mutex_);
        outgoing_.push_back(json + "\n");
    }

    void register_control(Control* c) { controls_[c->id_] = c; }

    bool refresh(Control& c) {
        bool changed = false;
        if (c.enabled_fn_) {
            bool e = c.enabled_fn_();
            if (e != c.enabled_) { c.enabled_ = e; changed = true; }
        }
        if (c.visible_fn_) {
            bool v = c.visible_fn_();
            if (v != c.visible_) { c.visible_ = v; changed = true; }
        }
        if (c.options_fn_) {
            c.options_ = c.options_fn_();
            json::Writer w;
            c.write_options(w);
            if (w.str() != c.sent_options_) { c.sent_options_ = w.str(); changed = true; }
        }
        return changed;
    }

    std::string build_layout(bool mark_sent) {
        json::Writer w;
        w.begin_object().key("type").string("layout").key("pages").begin_array();
        for (auto& p : pages_) {
            w.begin_object().key("id").string(p->id_).key("title");
            p->title_.write(w);
            if (!p->icon_.empty()) w.key("icon").string(p->icon_);
            w.key("sections").begin_array();
            for (auto& s : p->sections_) {
                w.begin_object().key("id").string(s->id_);
                if (!s->title_.empty()) { w.key("title"); s->title_.write(w); }
                if (!s->description_.empty()) { w.key("description"); s->description_.write(w); }
                w.key("controls").begin_array();
                for (auto& c : s->controls_) {
                    refresh(*c);
                    auto v = c->read_value();
                    if (mark_sent) c->sent_value_ = v;
                    c->write_layout(w, v ? &*v : nullptr);
                }
                w.end_array().end_object();
            }
            w.end_array().end_object();
        }
        w.end_array().end_object();
        return w.str();
    }

    void sync() {
        last_sync_ = now_ms();
        json::Writer values, patch;
        bool any_value = false, any_patch = false;
        values.begin_object().key("type").string("state").key("values").begin_object();
        patch.begin_object().key("type").string("patch").key("controls").begin_object();
        for (auto& kv : controls_) {
            Control& c = *kv.second;
            if (refresh(c)) {
                any_patch = true;
                patch.key(c.id_).begin_object().key("enabled").boolean(c.enabled_).key("visible").boolean(c.visible_);
                if (c.options_fn_) {
                    patch.key("options");
                    c.write_options(patch);
                }
                patch.end_object();
            }
            auto v = c.read_value();
            if (v && v != c.sent_value_) {
                c.sent_value_ = v;
                values.key(c.id_).raw(*v);
                any_value = true;
            }
        }
        if (any_patch) push(patch.end_object().end_object().str());
        if (any_value) push(values.end_object().end_object().str());
        sync_maps();
    }

    // карты: целиком (новое подключение, сменилось описание) или только изменившиеся метки
    void sync_maps() {
        if (!connected_) return;
        for (auto& m : maps_) {
            if (m->dirty_) {
                m->dirty_ = false;
                push(m->build());
                continue;
            }
            std::string p = m->build_patch();
            if (!p.empty()) push(p);
        }
    }

    void handle_map_action(const json::Value& msg) {
        double req = msg.get("req") ? msg.get("req")->as_number() : 0;
        std::string map_id = msg.get("map") ? msg.get("map")->s : std::string();
        std::string action = msg.get("action") ? msg.get("action")->s : std::string();
        Map* map = nullptr;
        for (auto& m : maps_)
            if (m->id_ == map_id) map = m.get();
        std::function<std::string(const MapHit&)>* handler = nullptr;
        if (map) {
            auto it = map->handlers_.find(action);
            if (it != map->handlers_.end()) handler = &it->second;
        }
        if (!handler) {
            reply(req, false, map_id + "/" + action + ": unknown");
            return;
        }
        MapHit hit;
        if (auto* mk = msg.get("marker")) hit.marker = mk->s;
        if (auto* x = msg.get("x")) hit.x = x->as_number();
        if (auto* y = msg.get("y")) hit.y = y->as_number();
        try {
            reply(req, true, (*handler)(hit));
        } catch (const std::exception& e) {
            reply(req, false, e.what());
        } catch (...) {
            reply(req, false, "error");
        }
        sync();
    }

    void reply(double req, bool ok, const std::string& text) {
        json::Writer w;
        w.begin_object().key("type").string("result").key("req").number(req).key("ok").boolean(ok);
        if (!text.empty()) w.key("text").string(text);
        w.end_object();
        push(w.str());
    }

    void handle(const std::string& line) {
        json::Value msg;
        if (!json::Parser(line).parse(msg) || msg.type != json::Value::Type::Object) return;
        const json::Value* type = msg.get("type");
        if (!type) return;
        const std::string& t = type->s;
        if (t == "welcome" || t == "lang") {
            if (auto* l = msg.get("lang")) {
                language_ = l->s;
                if (on_language) on_language(language_);
            }
        } else if (t == "set" || t == "invoke") {
            double req = msg.get("req") ? msg.get("req")->as_number() : 0;
            std::string id = msg.get("id") ? msg.get("id")->s : std::string();
            auto it = controls_.find(id);
            if (it == controls_.end()) {
                reply(req, false, id + ": unknown");
                return;
            }
            Control& c = *it->second;
            try {
                if (t == "set" && c.setter_) {
                    static const json::Value null_value;
                    const json::Value* v = msg.get("value");
                    c.setter_(v ? *v : null_value);
                    reply(req, true, {});
                } else if (t == "invoke" && c.action_) {
                    reply(req, true, c.action_());
                } else {
                    reply(req, false, id + ": not supported");
                }
            } catch (const std::exception& e) {
                reply(req, false, e.what());
            } catch (...) {
                reply(req, false, "error");
            }
            sync();
        } else if (t == "map_action") {
            handle_map_action(msg);
        } else if (t == "ping") {
            push(R"({"type":"pong"})");
        }
    }

    // ---------------------------------------------------------------- канал (фоновый поток)

    static bool write_all(HANDLE pipe, const std::string& data) {
        size_t offset = 0;
        while (offset < data.size()) {
            DWORD written = 0;
            if (!WriteFile(pipe, data.data() + offset, static_cast<DWORD>(data.size() - offset), &written, nullptr) || written == 0) return false;
            offset += written;
        }
        return true;
    }

    void io_loop() {
        std::string pending;
        std::vector<char> buffer(64 * 1024);
        while (!stop_) {
            HANDLE pipe = CreateFileW(pipe_name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, 0, nullptr);
            if (pipe == INVALID_HANDLE_VALUE) {
                for (int t = 0; t < 2000 && !stop_; t += 50) std::this_thread::sleep_for(std::chrono::milliseconds(50));
                continue;
            }
            bool ok = write_all(pipe, hello_);
            if (ok) {
                std::lock_guard<std::mutex> lock(cache_mutex_);
                // интерфейс, собранный заранее, — сразу, даже если игровой поток сейчас стоит
                if (!cached_layout_.empty()) ok = write_all(pipe, cached_layout_ + "\n");
            }
            if (ok) {
                {
                    std::lock_guard<std::mutex> lock(out_mutex_);
                    outgoing_.clear();
                }
                pending.clear();
                last_pump_.store(now_ms());
                connected_ = true;
                ++generation_;
                bool paused_sent = false;
                while (!stop_) {
                    std::deque<std::string> out;
                    {
                        std::lock_guard<std::mutex> lock(out_mutex_);
                        out.swap(outgoing_);
                    }
                    bool good = true;
                    for (auto& l : out)
                        if (!write_all(pipe, l)) { good = false; break; }
                    if (!good) break;
                    bool stalled = now_ms() - last_pump_.load() > 1500;
                    if (stalled != paused_sent) {
                        paused_sent = stalled;
                        if (!write_all(pipe, stalled ? "{\"type\":\"paused\",\"paused\":true}\n" : "{\"type\":\"paused\",\"paused\":false}\n")) break;
                    }
                    DWORD avail = 0;
                    if (!PeekNamedPipe(pipe, nullptr, 0, nullptr, &avail, nullptr)) break;
                    if (avail == 0) {
                        std::this_thread::sleep_for(std::chrono::milliseconds(15));
                        continue;
                    }
                    DWORD read = 0;
                    DWORD want = avail < buffer.size() ? avail : static_cast<DWORD>(buffer.size());
                    if (!ReadFile(pipe, buffer.data(), want, &read, nullptr) || read == 0) break;
                    for (DWORD i = 0; i < read; ++i) {
                        if (buffer[i] == '\n') {
                            if (!pending.empty()) {
                                std::lock_guard<std::mutex> lock(in_mutex_);
                                incoming_.push_back(pending);
                            }
                            pending.clear();
                        } else {
                            pending += buffer[i];
                        }
                    }
                    if (pending.size() > 4 * 1024 * 1024) break;
                }
                if (stop_) {
                    std::lock_guard<std::mutex> lock(out_mutex_);
                    for (auto& l : outgoing_) write_all(pipe, l);
                    outgoing_.clear();
                }
            }
            connected_ = false;
            CloseHandle(pipe);
        }
    }

    ModInfo info_;
    std::vector<std::unique_ptr<Page>> pages_;
    std::map<std::string, Control*> controls_;
    std::vector<std::unique_ptr<Map>> maps_;
    std::string hello_;
    std::string language_;
    std::thread thread_;
    std::atomic<bool> stop_{false};
    std::atomic<bool> connected_{false};
    std::atomic<int> generation_{0};
    std::atomic<long long> last_pump_{0};
    int sent_generation_ = 0;
    bool was_connected_ = false;
    long long last_sync_ = 0;
    long long cached_layout_time_ = 0;
    std::mutex out_mutex_, in_mutex_, cache_mutex_;
    std::deque<std::string> outgoing_, incoming_;
    std::string cached_layout_;
};

inline Control& Section::add(const std::string& id, const char* kind, Text label) {
    controls_.push_back(std::make_unique<Control>(id, kind, std::move(label)));
    client_.register_control(controls_.back().get());
    return *controls_.back();
}

}  // namespace modly
