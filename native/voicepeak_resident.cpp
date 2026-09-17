#include <cstdint>
#include <istream>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Args = std::vector<std::string>;

// Private stdin protocol: little-endian u32 argc, then u32 length + UTF-8 bytes.
static std::optional<Args> read_args(std::istream& input) {
    if (input.peek() == std::char_traits<char>::eof()) return std::nullopt;
    auto number = [&]() {
        unsigned char bytes[4];
        if (!input.read(reinterpret_cast<char*>(bytes), 4)) throw std::runtime_error("truncated request");
        return uint32_t(bytes[0]) | uint32_t(bytes[1]) << 8 | uint32_t(bytes[2]) << 16 | uint32_t(bytes[3]) << 24;
    };
    const auto count = number();
    if (count == 0 || count > 32) throw std::runtime_error("invalid argument count");
    Args args;
    uint32_t remaining = 1024 * 1024;
    for (uint32_t i = 0; i < count; ++i) {
        const auto size = number();
        if (size > remaining) throw std::runtime_error("request too large");
        remaining -= size;
        std::string arg(size, '\0');
        if (!input.read(arg.data(), size)) throw std::runtime_error("truncated argument");
        if (arg.find('\0') != std::string::npos) throw std::runtime_error("NUL in argument");
        args.push_back(std::move(arg));
    }
    return args;
}

#ifndef MOCA_RESIDENT_TEST
#include <cerrno>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <functional>
#include <iostream>
#include <link.h>
#include <memory>
#include <mutex>
#include <sys/mman.h>
#include <thread>
#include <unistd.h>

// Linux VOICEPEAK 1.2.21 only. Keep the original initialization, license checks,
// argument validation and renderer; retain its runtime between serial requests.
using Model = std::shared_ptr<void>;
using Done = std::function<void(bool)>;
using Cancel = std::function<void()>;
using Command = Cancel (*)(Model, Args, Done);
using Ready = std::function<void(Model)>;
using Failed = std::function<void(std::exception_ptr)>;
using Ensure = void (*)(Model, bool, Ready, Failed);
using Dispatch = bool (*)(std::function<void()>);
static Command original_command;
static Ensure original_ensure;
static Dispatch dispatch;
static Model model, runtime;
static Done finish;
static int response_fd;
static std::mutex mutex;
static std::condition_variable completion;
static bool busy;

static void respond(char value) {
    ssize_t n;
    do { n = write(response_fd, &value, 1); } while (n < 0 && errno == EINTR);
    if (n != 1) _exit(70);
}

static void ensure(Model incoming, bool flag, Ready ready, Failed failed) {
    if (!incoming) incoming = runtime;
    original_ensure(std::move(incoming), flag, [ready](Model value) {
        runtime = value;
        ready(std::move(value));
    }, std::move(failed));
}

static void read_requests() {
    try {
        while (auto args = read_args(std::cin)) {
            std::unique_lock<std::mutex> lock(mutex);
            busy = true;
            // All native object access stays on VOICEPEAK's message thread.
            if (!dispatch([args = std::move(*args)] {
                original_command(model, args, [](bool) {
                    // The native bool requests app termination, not success.
                    // The parent validates the fresh WAV after completion.
                    std::lock_guard<std::mutex> lock(mutex);
                    busy = false;
                    respond('D');
                    completion.notify_one();
                });
            })) _exit(71);
            completion.wait(lock, [] { return !busy; });
        }
        if (!dispatch([] {
            runtime.reset();
            model.reset();
            auto callback = std::move(finish);
            callback(true);
        })) _exit(71);
    } catch (const std::exception& e) {
        fprintf(stderr, "moca resident protocol: %s\n", e.what());
        _exit(72);
    }
}

static Cancel command(Model incoming, Args, Done callback) {
    model = std::move(incoming);
    finish = std::move(callback);
    std::thread(read_requests).detach();
    respond('R');
    return [] {};
}

static void jump(unsigned char* to, void* target) {
    const unsigned char op[] = {0xff, 0x25, 0, 0, 0, 0};
    memcpy(to, op, 6);
    memcpy(to + 6, &target, 8);
}

static void* patch(uintptr_t address, const unsigned char* expected, size_t size, void* hook) {
    auto entry = reinterpret_cast<unsigned char*>(address);
    if (memcmp(entry, expected, size)) _exit(73);
    auto trampoline = static_cast<unsigned char*>(mmap(nullptr, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    if (trampoline == MAP_FAILED) _exit(74);
    memcpy(trampoline, entry, size);
    jump(trampoline + size, entry + size);
    if (mprotect(trampoline, 4096, PROT_READ | PROT_EXEC)) _exit(74);
    void* page = reinterpret_cast<void*>(address & ~uintptr_t(4095));
    if (mprotect(page, 4096, PROT_READ | PROT_WRITE | PROT_EXEC)) _exit(74);
    jump(entry, hook);
    if (mprotect(page, 4096, PROT_READ | PROT_EXEC)) _exit(74);
    return trampoline;
}

__attribute__((constructor)) static void install() {
    uintptr_t base = 0;
    dl_iterate_phdr([](dl_phdr_info* info, size_t, void* p) {
        if (!*info->dlpi_name) { *static_cast<uintptr_t*>(p) = info->dlpi_addr; return 1; }
        return 0;
    }, &base);
    const unsigned char build_id[] = {0x65,0xa2,0xef,0xfb,0xee,0x1d,0x5d,0x90,0x8a,0x73,0x5d,0x1b,0x66,0xb4,0xaa,0xad,0x20,0x71,0xbb,0xc4};
    if (memcmp(reinterpret_cast<void*>(base+0x30c), build_id, sizeof(build_id))) _exit(73);
    const unsigned char command_entry[] = {0xf3,0x0f,0x1e,0xfa,0x41,0x57,0x41,0x56,0x41,0x55,0x41,0x54,0x49,0x89,0xd4,0x55};
    const unsigned char ensure_entry[] = {0x41,0x57,0x41,0x56,0x49,0x89,0xce,0x41,0x55,0x49,0x89,0xd5,0x41,0x54};
    original_command = reinterpret_cast<Command>(patch(base+0x220720, command_entry, sizeof(command_entry), reinterpret_cast<void*>(command)));
    original_ensure = reinterpret_cast<Ensure>(patch(base+0x218d70, ensure_entry, sizeof(ensure_entry), reinterpret_cast<void*>(ensure)));
    dispatch = reinterpret_cast<Dispatch>(base+0x6a9a70);
    // Reserve the parent's stdout pipe for protocol bytes; native logs go to stderr.
    response_fd = fcntl(STDOUT_FILENO, F_DUPFD_CLOEXEC, 3);
    if (response_fd < 0 || dup2(STDERR_FILENO, STDOUT_FILENO) < 0) _exit(74);
}
#endif

#ifdef MOCA_RESIDENT_TEST
#include <cassert>
#include <sstream>

int main() {
    std::string wire;
    auto number = [&](uint32_t n) {
        for (int i = 0; i < 4; ++i) wire += static_cast<char>(n >> (8 * i));
    };
    Args expected = {"voicepeak", "-s", "雨\nが降っています", "-o", "/tmp/a wav"};
    number(expected.size());
    for (const auto& arg : expected) { number(arg.size()); wire += arg; }
    std::istringstream input(wire);
    assert(read_args(input) == expected);
    assert(!read_args(input));
    auto rejects = [](std::string value) {
        std::istringstream input(value);
        try { read_args(input); return false; }
        catch (const std::runtime_error&) { return true; }
    };
    assert(rejects(wire.substr(0, wire.size() - 1)));
    assert(rejects(std::string("\1\0", 2)));
    assert(rejects(std::string("\0\0\0\0", 4)));
    assert(rejects(std::string("\xff\xff\xff\xff", 4)));
    assert(rejects(std::string("\1\0\0\0\xff\xff\xff\xff", 8)));
    assert(rejects(std::string("\1\0\0\0\1\0\0\0\0", 9)));
}
#endif
