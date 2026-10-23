// Modly SDK для C++ — обновления мода с mods.hackerstone.xyz. Один заголовок, C++17, только то,
// что есть в Windows: WinHTTP, BCrypt и tar.exe.
//
// Мод сам узнаёт о новой версии и ставит её — даже если Modly не запущен:
//
//   #include "modly_update.hpp"
//   static modly::update::Updater g_updates({
//       "my-mod",        // адрес мода на сайте: mods.hackerstone.xyz/m/my-mod
//       "1.0.0",         // своя версия
//       kAuthorKeyN,     // открытый ключ автора (hex, `cargo xtask mod-keygen` в репозитории Modly)
//   });
//   modly::update::RemoveOldFiles("my-mod"); // при запуске: убрать *.old от прошлого обновления
//   g_updates.Check();                     // в фоне; что нашлось — g_updates.Get()
//   g_updates.Install(gameRoot);           // в фоне: скачать, проверить, разложить файлы
//
// Чему верит мод:
//   1. Сайт отдаёт только выпуски, одобренные модератором (`/api/v1/mods/<адрес>`), и только по https.
//   2. Архив сверяется по SHA-256 из ответа сайта.
//   3. Рядом с архивом — подпись автора `<архив>.sig`: RSA (PKCS#1 v1.5, SHA-256) строки
//      "modly-mod-release\n<адрес>\n<версия>\n<sha256>\n". Без закрытого ключа автора её не подделать,
//      даже получив доступ к сайту. Подписывает `cargo xtask mod-sign <архив>`.
//   4. Файлы раскладываются по `modly.json` из архива (`install.files`: from → to, от папки игры). Файл,
//      занятый игрой (DLL мода), переименовывается в *.old — Windows это разрешает, — а новый
//      включится при следующем запуске игры.
//
// Строки — UTF-8. Подробнее — docs/MOD-UPDATES.md в репозитории Modly.

#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <bcrypt.h>
#include <winhttp.h>

#include <atomic>
#include <cctype>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#pragma comment(lib, "winhttp.lib")
#pragma comment(lib, "bcrypt.lib")

namespace modly::update {

inline constexpr const char* kSite = "https://mods.hackerstone.xyz";

// ------------------------------------------------------------------ строки ---

inline std::wstring Wide(const std::string& s) {
    if (s.empty()) return {};
    int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
    std::wstring w(n, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), w.data(), n);
    return w;
}

inline std::string Narrow(const std::wstring& w) {
    if (w.empty()) return {};
    int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
    std::string s(n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), s.data(), n, nullptr, nullptr);
    return s;
}

inline std::string Hex(const uint8_t* p, size_t n) {
    static const char* d = "0123456789abcdef";
    std::string s;
    s.reserve(n * 2);
    for (size_t i = 0; i < n; ++i) {
        s.push_back(d[p[i] >> 4]);
        s.push_back(d[p[i] & 15]);
    }
    return s;
}

inline bool Unhex(const std::string& s, std::vector<uint8_t>& out) {
    auto v = [](char c) -> int {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return -1;
    };
    std::string t;
    for (char c : s) {
        if (!isspace((unsigned char)c)) t.push_back(c);
    }
    if (t.size() % 2) return false;
    out.clear();
    for (size_t i = 0; i < t.size(); i += 2) {
        int a = v(t[i]), b = v(t[i + 1]);
        if (a < 0 || b < 0) return false;
        out.push_back(uint8_t(a * 16 + b));
    }
    return true;
}

// -------------------------------------------------------------------- JSON ---
// Небольшой разборщик JSON: ответы сайта и modly.json.

struct Json {
    enum Type { Null, Bool, Number, String, Array, Object } type = Null;
    bool b = false;
    double num = 0;
    std::string str;
    std::vector<Json> arr;
    std::map<std::string, Json> obj;

    const Json& operator[](const char* key) const {
        static const Json none;
        if (type != Object) return none;
        auto it = obj.find(key);
        return it == obj.end() ? none : it->second;
    }
    std::string Str(const std::string& def = {}) const { return type == String ? str : def; }
    bool True() const { return type == Bool && b; }

    static bool Parse(const std::string& text, Json& out) {
        size_t i = 0;
        return Value(text, i, out, 0) && (Skip(text, i), i == text.size());
    }

private:
    static void Skip(const std::string& s, size_t& i) {
        while (i < s.size() && (s[i] == ' ' || s[i] == '\t' || s[i] == '\n' || s[i] == '\r')) ++i;
    }
    static void Utf8(std::string& o, uint32_t c) {
        if (c < 0x80) o.push_back(char(c));
        else if (c < 0x800) { o.push_back(char(0xC0 | (c >> 6))); o.push_back(char(0x80 | (c & 63))); }
        else if (c < 0x10000) { o.push_back(char(0xE0 | (c >> 12))); o.push_back(char(0x80 | ((c >> 6) & 63))); o.push_back(char(0x80 | (c & 63))); }
        else { o.push_back(char(0xF0 | (c >> 18))); o.push_back(char(0x80 | ((c >> 12) & 63))); o.push_back(char(0x80 | ((c >> 6) & 63))); o.push_back(char(0x80 | (c & 63))); }
    }
    static bool Hex4(const std::string& s, size_t i, uint32_t& c) {
        if (i + 4 > s.size()) return false;
        c = 0;
        for (size_t k = 0; k < 4; ++k) {
            char h = s[i + k];
            c <<= 4;
            if (h >= '0' && h <= '9') c |= h - '0';
            else if (h >= 'a' && h <= 'f') c |= h - 'a' + 10;
            else if (h >= 'A' && h <= 'F') c |= h - 'A' + 10;
            else return false;
        }
        return true;
    }
    static bool ParseStr(const std::string& s, size_t& i, std::string& out) {
        if (s[i] != '"') return false;
        ++i;
        while (i < s.size()) {
            char c = s[i++];
            if (c == '"') return true;
            if (c != '\\') { out.push_back(c); continue; }
            if (i >= s.size()) return false;
            char e = s[i++];
            switch (e) {
                case '"': out.push_back('"'); break;
                case '\\': out.push_back('\\'); break;
                case '/': out.push_back('/'); break;
                case 'b': out.push_back('\b'); break;
                case 'f': out.push_back('\f'); break;
                case 'n': out.push_back('\n'); break;
                case 'r': out.push_back('\r'); break;
                case 't': out.push_back('\t'); break;
                case 'u': {
                    uint32_t c1;
                    if (!Hex4(s, i, c1)) return false;
                    i += 4;
                    if (c1 >= 0xD800 && c1 < 0xDC00 && i + 6 <= s.size() && s[i] == '\\' && s[i + 1] == 'u') {
                        uint32_t c2;
                        if (Hex4(s, i + 2, c2) && c2 >= 0xDC00 && c2 < 0xE000) {
                            i += 6;
                            c1 = 0x10000 + ((c1 - 0xD800) << 10) + (c2 - 0xDC00);
                        }
                    }
                    Utf8(out, c1);
                    break;
                }
                default: return false;
            }
        }
        return false;
    }
    static bool Value(const std::string& s, size_t& i, Json& v, int depth) {
        if (depth > 64) return false;
        Skip(s, i);
        if (i >= s.size()) return false;
        char c = s[i];
        if (c == '{') {
            v.type = Object;
            ++i;
            Skip(s, i);
            if (i < s.size() && s[i] == '}') { ++i; return true; }
            while (i < s.size()) {
                Skip(s, i);
                std::string key;
                if (i >= s.size() || !ParseStr(s, i, key)) return false;
                Skip(s, i);
                if (i >= s.size() || s[i++] != ':') return false;
                Json item;
                if (!Value(s, i, item, depth + 1)) return false;
                v.obj[key] = std::move(item);
                Skip(s, i);
                if (i < s.size() && s[i] == ',') { ++i; continue; }
                if (i < s.size() && s[i] == '}') { ++i; return true; }
                return false;
            }
            return false;
        }
        if (c == '[') {
            v.type = Array;
            ++i;
            Skip(s, i);
            if (i < s.size() && s[i] == ']') { ++i; return true; }
            while (i < s.size()) {
                Json item;
                if (!Value(s, i, item, depth + 1)) return false;
                v.arr.push_back(std::move(item));
                Skip(s, i);
                if (i < s.size() && s[i] == ',') { ++i; continue; }
                if (i < s.size() && s[i] == ']') { ++i; return true; }
                return false;
            }
            return false;
        }
        if (c == '"') { v.type = String; return ParseStr(s, i, v.str); }
        if (s.compare(i, 4, "true") == 0) { v.type = Bool; v.b = true; i += 4; return true; }
        if (s.compare(i, 5, "false") == 0) { v.type = Bool; i += 5; return true; }
        if (s.compare(i, 4, "null") == 0) { i += 4; return true; }
        size_t start = i;
        while (i < s.size() && (isdigit((unsigned char)s[i]) || s[i] == '-' || s[i] == '+' || s[i] == '.' || s[i] == 'e' || s[i] == 'E')) ++i;
        if (i == start) return false;
        v.type = Number;
        v.num = atof(s.substr(start, i - start).c_str());
        return true;
    }
};

// ------------------------------------------------------------------ версии ---

/// Сравнить версии вида 1.2.3 и 1.2.3-beta.2 (как в SemVer: 1.0.0-beta < 1.0.0). <0, 0, >0.
inline int CompareVersions(const std::string& a, const std::string& b) {
    auto split = [](std::string v, std::vector<long long>& nums, std::vector<std::string>& pre) {
        if (!v.empty() && (v[0] == 'v' || v[0] == 'V')) v.erase(0, 1);
        v = v.substr(0, v.find('+'));
        size_t dash = v.find('-');
        std::string core = v.substr(0, dash), p = dash == std::string::npos ? "" : v.substr(dash + 1);
        size_t s = 0;
        while (s <= core.size()) {
            size_t e = core.find('.', s);
            nums.push_back(atoll(core.substr(s, e - s).c_str()));
            if (e == std::string::npos) break;
            s = e + 1;
        }
        nums.resize(3, 0);
        s = 0;
        while (!p.empty() && s <= p.size()) {
            size_t e = p.find('.', s);
            pre.push_back(p.substr(s, e - s));
            if (e == std::string::npos) break;
            s = e + 1;
        }
    };
    std::vector<long long> na, nb;
    std::vector<std::string> pa, pb;
    split(a, na, pa);
    split(b, nb, pb);
    for (size_t i = 0; i < 3; ++i) {
        if (na[i] != nb[i]) return na[i] < nb[i] ? -1 : 1;
    }
    if (pa.empty() || pb.empty()) return pa.empty() == pb.empty() ? 0 : (pa.empty() ? 1 : -1);
    auto num = [](const std::string& s) { return !s.empty() && s.find_first_not_of("0123456789") == std::string::npos; };
    for (size_t i = 0; i < pa.size() && i < pb.size(); ++i) {
        bool x = num(pa[i]), y = num(pb[i]);
        int c = x && y ? (atoll(pa[i].c_str()) < atoll(pb[i].c_str()) ? -1 : atoll(pa[i].c_str()) > atoll(pb[i].c_str()) ? 1 : 0)
              : x ? -1 : y ? 1 : pa[i].compare(pb[i]);
        if (c) return c < 0 ? -1 : 1;
    }
    return pa.size() == pb.size() ? 0 : (pa.size() < pb.size() ? -1 : 1);
}

// ------------------------------------------------------------------ крипто ---

/// SHA-256 через BCrypt.
class Sha256 {
public:
    Sha256() {
        if (BCryptOpenAlgorithmProvider(&alg_, BCRYPT_SHA256_ALGORITHM, nullptr, 0) == 0) {
            BCryptCreateHash(alg_, &hash_, nullptr, 0, nullptr, 0, 0);
        }
    }
    ~Sha256() {
        if (hash_) BCryptDestroyHash(hash_);
        if (alg_) BCryptCloseAlgorithmProvider(alg_, 0);
    }
    Sha256(const Sha256&) = delete;
    Sha256& operator=(const Sha256&) = delete;
    void Add(const void* p, size_t n) {
        if (hash_) BCryptHashData(hash_, (PUCHAR)p, (ULONG)n, 0);
    }
    std::vector<uint8_t> Done() {
        std::vector<uint8_t> out(32);
        if (!hash_ || BCryptFinishHash(hash_, out.data(), 32, 0) != 0) out.clear();
        return out;
    }

private:
    BCRYPT_ALG_HANDLE alg_ = nullptr;
    BCRYPT_HASH_HANDLE hash_ = nullptr;
};

/// Проверить подпись RSA PKCS#1 v1.5 + SHA-256. Ключ — модуль и экспонента (hex, big-endian).
inline bool VerifyRsa(const std::string& message, const std::vector<uint8_t>& sig, const std::string& modulusHex, const std::string& exponentHex) {
    std::vector<uint8_t> n, e;
    if (!Unhex(modulusHex, n) || !Unhex(exponentHex, e) || n.empty() || e.empty()) return false;
    while (n.size() > 1 && n[0] == 0) n.erase(n.begin());
    std::vector<uint8_t> blob(sizeof(BCRYPT_RSAKEY_BLOB) + e.size() + n.size());
    auto* h = reinterpret_cast<BCRYPT_RSAKEY_BLOB*>(blob.data());
    h->Magic = BCRYPT_RSAPUBLIC_MAGIC;
    h->BitLength = ULONG(n.size() * 8);
    h->cbPublicExp = ULONG(e.size());
    h->cbModulus = ULONG(n.size());
    memcpy(blob.data() + sizeof(BCRYPT_RSAKEY_BLOB), e.data(), e.size());
    memcpy(blob.data() + sizeof(BCRYPT_RSAKEY_BLOB) + e.size(), n.data(), n.size());

    Sha256 sha;
    sha.Add(message.data(), message.size());
    std::vector<uint8_t> digest = sha.Done();
    if (digest.size() != 32) return false;

    BCRYPT_ALG_HANDLE alg = nullptr;
    if (BCryptOpenAlgorithmProvider(&alg, BCRYPT_RSA_ALGORITHM, nullptr, 0) != 0) return false;
    BCRYPT_KEY_HANDLE key = nullptr;
    bool ok = false;
    if (BCryptImportKeyPair(alg, nullptr, BCRYPT_RSAPUBLIC_BLOB, &key, blob.data(), (ULONG)blob.size(), 0) == 0) {
        BCRYPT_PKCS1_PADDING_INFO pad = {BCRYPT_SHA256_ALGORITHM};
        ok = BCryptVerifySignature(key, &pad, digest.data(), 32, (PUCHAR)sig.data(), (ULONG)sig.size(), BCRYPT_PAD_PKCS1) == 0;
        BCryptDestroyKey(key);
    }
    BCryptCloseAlgorithmProvider(alg, 0);
    return ok;
}

/// Строка, которую подписывает автор: адрес мода, версия и SHA-256 архива.
inline std::string SignedMessage(const std::string& slug, const std::string& version, const std::string& sha256) {
    std::string s = sha256;
    for (char& c : s) c = (char)tolower((unsigned char)c);
    return "modly-mod-release\n" + slug + "\n" + version + "\n" + s + "\n";
}

// -------------------------------------------------------------------- сеть ---

struct Url {
    bool https = true;
    std::wstring host, path;
    INTERNET_PORT port = 443;
};

inline bool ParseUrl(const std::string& url, Url& u) {
    std::wstring w = Wide(url);
    URL_COMPONENTS c = {sizeof(c)};
    wchar_t host[256] = {}, path[2048] = {}, extra[2048] = {};
    c.lpszHostName = host;
    c.dwHostNameLength = 256;
    c.lpszUrlPath = path;
    c.dwUrlPathLength = 2048;
    c.lpszExtraInfo = extra;
    c.dwExtraInfoLength = 2048;
    if (!WinHttpCrackUrl(w.c_str(), 0, 0, &c)) return false;
    u.https = c.nScheme == INTERNET_SCHEME_HTTPS;
    u.host = host;
    u.port = c.nPort;
    u.path = std::wstring(path) + extra;
    return !u.host.empty();
}

inline bool IsLoopback(const std::wstring& host) {
    return host == L"127.0.0.1" || _wcsicmp(host.c_str(), L"localhost") == 0;
}

/// GET. Тело — в `body` (не больше `limit`) или в `file`; `onData` видит каждый кусок.
/// Только https (http — лишь к своему компьютеру), только на хост `allowedHost`, без перенаправлений.
inline bool HttpGet(const std::string& url, const std::wstring& allowedHost, std::string* body, size_t limit, HANDLE file,
                    const std::function<bool(const uint8_t*, size_t, uint64_t)>& onData, std::string& err) {
    Url u;
    if (!ParseUrl(url, u)) { err = "bad url"; return false; }
    if (_wcsicmp(u.host.c_str(), allowedHost.c_str()) != 0) { err = "foreign host"; return false; }
    if (!u.https && !IsLoopback(u.host)) { err = "https only"; return false; }
    HINTERNET s = WinHttpOpen(L"Modly-Mod-Updater/1.0", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (!s) { err = "WinHttpOpen"; return false; }
    WinHttpSetTimeouts(s, 15000, 15000, 30000, 60000);
    HINTERNET c = WinHttpConnect(s, u.host.c_str(), u.port, 0);
    HINTERNET r = c ? WinHttpOpenRequest(c, L"GET", u.path.c_str(), nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, u.https ? WINHTTP_FLAG_SECURE : 0) : nullptr;
    bool ok = false;
    if (r) {
        DWORD never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        WinHttpSetOption(r, WINHTTP_OPTION_REDIRECT_POLICY, &never, sizeof(never));
        if (WinHttpSendRequest(r, L"Accept: application/json, */*\r\n", (DWORD)-1L, WINHTTP_NO_REQUEST_DATA, 0, 0, 0) && WinHttpReceiveResponse(r, nullptr)) {
            DWORD status = 0, len = sizeof(status);
            WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX, &status, &len, WINHTTP_NO_HEADER_INDEX);
            if (status != 200) {
                err = "HTTP " + std::to_string(status);
            } else {
                ok = true;
                uint64_t total = 0;
                std::vector<uint8_t> buf(64 * 1024);
                for (;;) {
                    DWORD got = 0;
                    if (!WinHttpReadData(r, buf.data(), (DWORD)buf.size(), &got)) { ok = false; err = "read failed"; break; }
                    if (!got) break;
                    total += got;
                    if (body) {
                        if (body->size() + got > limit) { ok = false; err = "response too big"; break; }
                        body->append((const char*)buf.data(), got);
                    }
                    if (file != INVALID_HANDLE_VALUE && file) {
                        DWORD wrote = 0;
                        if (!WriteFile(file, buf.data(), got, &wrote, nullptr) || wrote != got) { ok = false; err = "write failed"; break; }
                    }
                    if (onData && !onData(buf.data(), got, total)) { ok = false; err = "cancelled"; break; }
                }
            }
        } else {
            err = "no connection (" + std::to_string(GetLastError()) + ")";
        }
    } else if (err.empty()) {
        err = "connect failed";
    }
    if (r) WinHttpCloseHandle(r);
    if (c) WinHttpCloseHandle(c);
    WinHttpCloseHandle(s);
    return ok;
}

// ------------------------------------------------------------------- файлы ---

/// Путь из modly.json: только относительный и без «..».
inline bool SafeRelative(const std::string& p) {
    if (p.empty() || p[0] == '/' || p[0] == '\\' || p.find(':') != std::string::npos) return false;
    size_t s = 0;
    while (s <= p.size()) {
        size_t e = p.find_first_of("/\\", s);
        std::string part = p.substr(s, e - s);
        if (part == "..") return false;
        if (e == std::string::npos) break;
        s = e + 1;
    }
    return true;
}

inline std::wstring Join(const std::wstring& dir, const std::string& rel) {
    std::wstring r = Wide(rel);
    for (auto& ch : r) if (ch == L'/') ch = L'\\';
    return dir.empty() || dir.back() == L'\\' ? dir + r : dir + L"\\" + r;
}

inline void MakeDirs(const std::wstring& path) {
    for (size_t i = 3; i < path.size(); ++i) {
        if (path[i] == L'\\') CreateDirectoryW(path.substr(0, i).c_str(), nullptr);
    }
    CreateDirectoryW(path.c_str(), nullptr);
}

inline bool ReadFileText(const std::wstring& path, std::string& out) {
    HANDLE f = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
    if (f == INVALID_HANDLE_VALUE) return false;
    LARGE_INTEGER sz = {};
    GetFileSizeEx(f, &sz);
    out.resize((size_t)sz.QuadPart);
    DWORD got = 0;
    bool ok = sz.QuadPart < (1 << 24) && ReadFile(f, out.data(), (DWORD)out.size(), &got, nullptr) && got == out.size();
    CloseHandle(f);
    if (out.size() >= 3 && (uint8_t)out[0] == 0xEF && (uint8_t)out[1] == 0xBB && (uint8_t)out[2] == 0xBF) out.erase(0, 3);
    return ok;
}

/// Распаковать ZIP встроенным в Windows 10/11 tar.exe (bsdtar не выпускает файлы за папку).
inline bool ExtractZip(const std::wstring& zip, const std::wstring& dest, std::string& err) {
    wchar_t sys[MAX_PATH] = {};
    GetSystemDirectoryW(sys, MAX_PATH);
    std::wstring cmd = L"\"" + std::wstring(sys) + L"\\tar.exe\" -xf \"" + zip + L"\" -C \"" + dest + L"\"";
    STARTUPINFOW si = {sizeof(si)};
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE;
    PROCESS_INFORMATION pi = {};
    if (!CreateProcessW(nullptr, cmd.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr, nullptr, &si, &pi)) {
        err = "tar.exe not started";
        return false;
    }
    DWORD code = 1;
    if (WaitForSingleObject(pi.hProcess, 120000) == WAIT_OBJECT_0) GetExitCodeProcess(pi.hProcess, &code);
    else TerminateProcess(pi.hProcess, 1);
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    if (code != 0) err = "unpack failed";
    return code == 0;
}

/// Папка обновлений мода: `%LOCALAPPDATA%\Modly\mod-updates\<адрес>`.
inline std::wstring ModUpdatesDir(const std::string& slug) {
    wchar_t buf[MAX_PATH] = {};
    GetEnvironmentVariableW(L"LOCALAPPDATA", buf, MAX_PATH);
    return std::wstring(buf) + L"\\Modly\\mod-updates\\" + Wide(slug);
}

/// Старые файлы, которые обновление переименовало в *.old (их держала игра), — список в old-files.txt.
inline void RemoveOldFiles(const std::string& slug) {
    std::wstring list = ModUpdatesDir(slug) + L"\\old-files.txt";
    std::string text;
    if (!ReadFileText(list, text)) return;
    std::string keep;
    size_t s = 0;
    while (s < text.size()) {
        size_t e = text.find('\n', s);
        std::string line = text.substr(s, e - s);
        s = e == std::string::npos ? text.size() : e + 1;
        if (!line.empty() && line.back() == '\r') line.pop_back();
        if (line.empty()) continue;
        std::wstring path = Wide(line);
        if (GetFileAttributesW(path.c_str()) != INVALID_FILE_ATTRIBUTES && !DeleteFileW(path.c_str())) keep += line + "\n";
    }
    if (keep.empty()) {
        DeleteFileW(list.c_str());
        return;
    }
    HANDLE f = CreateFileW(list.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (f == INVALID_HANDLE_VALUE) return;
    DWORD w = 0;
    WriteFile(f, keep.data(), (DWORD)keep.size(), &w, nullptr);
    CloseHandle(f);
}

/// Дописать в old-files.txt, что убрать при следующем запуске.
inline void RememberOldFiles(const std::string& slug, const std::vector<std::wstring>& olds) {
    if (olds.empty()) return;
    std::wstring dir = ModUpdatesDir(slug);
    MakeDirs(dir);
    HANDLE f = CreateFileW((dir + L"\\old-files.txt").c_str(), FILE_APPEND_DATA, 0, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (f == INVALID_HANDLE_VALUE) return;
    for (const auto& o : olds) {
        std::string line = Narrow(o) + "\n";
        DWORD w = 0;
        WriteFile(f, line.data(), (DWORD)line.size(), &w, nullptr);
    }
    CloseHandle(f);
}

/// Положить файлы: занятые — в *.old (их список — в `olds`), при ошибке вернуть всё как было.
inline bool SwapFiles(const std::vector<std::pair<std::wstring, std::wstring>>& files, std::string& err, std::vector<std::wstring>* olds = nullptr) {
    std::vector<std::pair<std::wstring, std::wstring>> moved; // (файл, куда ушёл старый)
    std::vector<std::wstring> created;
    for (const auto& [src, dst] : files) {
        size_t slash = dst.find_last_of(L'\\');
        if (slash != std::wstring::npos) MakeDirs(dst.substr(0, slash));
        if (GetFileAttributesW(dst.c_str()) != INVALID_FILE_ATTRIBUTES) {
            std::wstring old = dst + L".old";
            if (!MoveFileExW(dst.c_str(), old.c_str(), MOVEFILE_REPLACE_EXISTING)) {
                old = dst + L".old" + std::to_wstring(GetTickCount64());
                if (!MoveFileExW(dst.c_str(), old.c_str(), 0)) { err = "file is busy: " + Narrow(dst); break; }
            }
            moved.push_back({dst, old});
        } else {
            created.push_back(dst);
        }
        if (!CopyFileW(src.c_str(), dst.c_str(), FALSE)) { err = "copy failed: " + Narrow(dst); break; }
    }
    if (err.empty()) {
        if (olds) for (const auto& m : moved) olds->push_back(m.second);
        return true;
    }
    for (const auto& c : created) DeleteFileW(c.c_str());
    for (auto it = moved.rbegin(); it != moved.rend(); ++it) {
        DeleteFileW(it->first.c_str());
        MoveFileExW(it->second.c_str(), it->first.c_str(), MOVEFILE_REPLACE_EXISTING);
    }
    return false;
}

// ----------------------------------------------------------------- обновитель ---

struct Config {
    std::string slug;            // адрес мода на сайте
    std::string version;         // своя версия
    std::string keyModulus;      // открытый ключ автора: модуль RSA (hex)
    std::string keyExponent = "010001";
    std::string site = kSite;    // другой адрес — только http(s)://127.0.0.1 для проверки
};

enum class State { Idle, Checking, Latest, Available, Downloading, Installed, Failed };

struct Status {
    State state = State::Idle;
    std::string version;   // найденная версия
    std::string notesRu, notesEn;
    std::string page;      // страница мода на сайте
    std::string error;
    float progress = 0;    // 0..1 — пока скачивается
    uint64_t checkedAt = 0; // GetTickCount64() последней проверки
};

class Updater {
public:
    explicit Updater(Config cfg) : s_(std::make_shared<Shared>()) { s_->cfg = std::move(cfg); }

    Status Get() const {
        std::lock_guard<std::mutex> l(s_->m);
        return s_->st;
    }
    bool Busy() const { return s_->busy; }

    /// Проверить в фоне, есть ли новая одобренная версия.
    void Check() {
        if (s_->busy.exchange(true)) return;
        auto s = s_;
        std::thread([s] { CheckNow(*s); s->busy = false; }).detach();
    }

    /// То же, но сразу (для своего фонового потока). true — есть новая версия.
    bool CheckSync() {
        if (s_->busy.exchange(true)) return false;
        bool found = CheckNow(*s_);
        s_->busy = false;
        return found;
    }

    /// Скачать найденную версию, проверить SHA-256 и подпись автора и разложить файлы от папки игры `gameRoot`.
    void Install(const std::wstring& gameRoot) {
        if (s_->busy.exchange(true)) return;
        auto s = s_;
        std::thread([s, gameRoot] { InstallNow(*s, gameRoot); s->busy = false; }).detach();
    }

private:
    struct Found {
        std::string version, zipUrl, zipName, sha256, sigUrl;
        uint64_t size = 0;
    };
    struct Shared {
        Config cfg;
        std::mutex m;
        Status st;
        Found found;
        std::atomic<bool> busy{false};
    };

    static std::wstring Host(const Config& c) {
        Url u;
        return ParseUrl(c.site, u) ? u.host : L"";
    }
    static void Set(Shared& s, State st, const std::string& error = {}) {
        std::lock_guard<std::mutex> l(s.m);
        s.st.state = st;
        s.st.error = error;
    }

    static bool CheckNow(Shared& s) {
        Set(s, State::Checking);
        std::string body, err;
        std::wstring host = Host(s.cfg);
        if (host.empty() || !HttpGet(s.cfg.site + "/api/v1/mods/" + s.cfg.slug, host, &body, 4u << 20, nullptr, nullptr, err)) {
            Set(s, State::Failed, "site: " + err);
            return false;
        }
        Json root;
        if (!Json::Parse(body, root) || !root["ok"].True()) {
            Set(s, State::Failed, "site: bad answer");
            return false;
        }
        const Json& mod = root["mod"];
        Found best;
        std::string notesRu, notesEn;
        for (const Json& r : mod["releases"].arr) {
            std::string status = r["status"].Str("approved");
            std::string ver = r["version"].Str();
            if (status != "approved" || ver.empty() || CompareVersions(ver, s.cfg.version) <= 0) continue;
            if (!best.version.empty() && CompareVersions(ver, best.version) <= 0) continue;
            Found f;
            f.version = ver;
            for (const Json& file : r["files"].arr) {
                std::string name = file["name"].Str();
                if (file["modly"].True() && file["kind"].Str() == "zip") {
                    f.zipName = name;
                    f.zipUrl = file["url"].Str();
                    f.sha256 = file["sha256"].Str();
                    f.size = (uint64_t)file["size"].num;
                }
            }
            for (const Json& file : r["files"].arr) {
                if (!f.zipName.empty() && file["name"].Str() == f.zipName + ".sig") f.sigUrl = file["url"].Str();
            }
            if (f.zipUrl.empty() || f.sigUrl.empty()) continue; // без пакета или без подписи автора — не ставим
            best = f;
            notesRu = r["notes"]["ru"].Str();
            notesEn = r["notes"]["en"].Str();
        }
        std::lock_guard<std::mutex> l(s.m);
        s.st.checkedAt = GetTickCount64();
        s.st.page = mod["url"].Str(s.cfg.site + "/m/" + s.cfg.slug);
        s.st.error.clear();
        if (best.version.empty()) {
            s.st.state = State::Latest;
            return false;
        }
        s.found = best;
        s.st.state = State::Available;
        s.st.version = best.version;
        s.st.notesRu = notesRu;
        s.st.notesEn = notesEn;
        s.st.progress = 0;
        return true;
    }

    static std::wstring WorkDir(const Config& c, const std::string& version) {
        return ModUpdatesDir(c.slug) + L"\\" + Wide(version);
    }

    static void InstallNow(Shared& s, const std::wstring& gameRoot) {
        Found f;
        {
            std::lock_guard<std::mutex> l(s.m);
            if (s.st.state != State::Available && s.st.state != State::Failed) return;
            f = s.found;
            s.st.state = State::Downloading;
            s.st.progress = 0;
            s.st.error.clear();
        }
        if (f.version.empty()) { Set(s, State::Failed, "nothing to install"); return; }
        std::wstring host = Host(s.cfg), dir = WorkDir(s.cfg, f.version);
        MakeDirs(dir);
        std::wstring zip = dir + L"\\" + Wide(f.zipName);
        std::string err;

        // архив: скачать и посчитать SHA-256
        HANDLE file = CreateFileW(zip.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (file == INVALID_HANDLE_VALUE) { Set(s, State::Failed, "can't write " + Narrow(zip)); return; }
        Sha256 sha;
        bool ok = HttpGet(f.zipUrl, host, nullptr, 0, file, [&](const uint8_t* p, size_t n, uint64_t total) {
            sha.Add(p, n);
            if (total > f.size + 1) return false;
            std::lock_guard<std::mutex> l(s.m);
            s.st.progress = f.size ? float(double(total) / double(f.size)) * 0.9f : 0.5f;
            return true;
        }, err);
        CloseHandle(file);
        std::vector<uint8_t> digest = sha.Done();
        std::string got = digest.empty() ? "" : Hex(digest.data(), digest.size());
        if (!ok) { Set(s, State::Failed, "download: " + err); return; }
        if (_stricmp(got.c_str(), f.sha256.c_str()) != 0) { Set(s, State::Failed, "SHA-256 mismatch"); return; }

        // подпись автора
        std::string sigText;
        std::vector<uint8_t> sig;
        if (!HttpGet(f.sigUrl, host, &sigText, 16384, nullptr, nullptr, err) || !Unhex(sigText, sig)) {
            Set(s, State::Failed, "signature: " + (err.empty() ? std::string("bad file") : err));
            return;
        }
        if (!VerifyRsa(SignedMessage(s.cfg.slug, f.version, got), sig, s.cfg.keyModulus, s.cfg.keyExponent)) {
            Set(s, State::Failed, "author signature is not valid");
            return;
        }

        // распаковать и разложить по modly.json
        std::wstring out = dir + L"\\files";
        MakeDirs(out);
        if (!ExtractZip(zip, out, err)) { Set(s, State::Failed, err); return; }
        std::string manifest;
        Json m;
        if (!ReadFileText(out + L"\\modly.json", manifest) || !Json::Parse(manifest, m)) { Set(s, State::Failed, "modly.json missing"); return; }
        // `install.files` (способ установки `files`, docs/MANIFEST.md); пути — от папки игры,
        // подстановки вида {plugins} здесь не поддерживаются
        const Json& list = m["install"]["files"].type == Json::Array ? m["install"]["files"] : m["files"];
        std::vector<std::pair<std::wstring, std::wstring>> files;
        for (const Json& e : list.arr) {
            std::string from = e["from"].Str(), to = e["to"].Str(from);
            if (!SafeRelative(from) || !SafeRelative(to) || to.find('{') != std::string::npos) {
                Set(s, State::Failed, "unsupported path in modly.json: " + to);
                return;
            }
            files.push_back({Join(out, from), Join(gameRoot, to)});
        }
        if (files.empty()) { Set(s, State::Failed, "modly.json has no files"); return; }
        std::vector<std::wstring> olds;
        if (!SwapFiles(files, err, &olds)) { Set(s, State::Failed, err); return; }
        RememberOldFiles(s.cfg.slug, olds);
        std::lock_guard<std::mutex> l(s.m);
        s.st.state = State::Installed;
        s.st.progress = 1;
    }

    std::shared_ptr<Shared> s_;
};

} // namespace modly::update
