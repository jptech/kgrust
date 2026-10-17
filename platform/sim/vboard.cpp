// The virtual board's front panel: a simulated design's LEDs, buttons,
// switches, serial console and screen, in a browser. A board's bridge
// testbench (virtual/sim/vboard.sv) calls these through Verilator's DPI-C as
// the simulation runs; a thread here serves vboard.html and the panel's
// state over HTTP on 127.0.0.1, at the port it writes to the file named by
// `vb_open`'s second argument (`kg board serve` prints the address).
//
// The simulation never waits for the browser. State is shared under one
// mutex: the testbench's calls set it, the page polls it.
//
// HTTP, one request per connection:
//   GET  /             the page
//   GET  /state?uart=N the panel as JSON, with the serial output from byte N
//   GET  /frame        the last whole frame: width x height pixels of 12 bits,
//                      two bytes each, little-endian {r, g, b}
//   POST /input?buttons=B&switches=S
//   POST /uart         the body's bytes, to the design's serial input

#include <arpa/inet.h>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <mutex>
#include <netinet/in.h>
#include <string>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

constexpr int WIDTH = 640;
constexpr int HEIGHT = 480;

struct Panel {
    std::mutex lock;
    std::string page;
    uint32_t leds[4] = {0, 0, 0, 0};     // by the testbench's index: 0 rgb, 1 leds
    uint32_t inputs[4] = {0, 0, 0, 0};   // 0 buttons, 1 switches
    std::vector<uint8_t> uart_out;      // everything the design has sent
    std::deque<uint8_t> uart_in;        // typed in the page, not yet sent
    std::vector<uint16_t> drawing = std::vector<uint16_t>(WIDTH * HEIGHT);
    std::vector<uint16_t> shown = std::vector<uint16_t>(WIDTH * HEIGHT);
    uint64_t frames = 0;
    uint64_t cycles = 0;
    double fps = 0, cycles_per_second = 0;
    std::chrono::steady_clock::time_point last_frame = std::chrono::steady_clock::now();
    std::chrono::steady_clock::time_point last_rate = std::chrono::steady_clock::now();
    uint64_t cycles_at_rate = 0;
    std::atomic<bool> screen{false};    // the board has drawn a pixel
};

Panel panel;

std::string query(const std::string& target, const std::string& key) {
    auto q = target.find('?');
    if (q == std::string::npos) return "";
    std::string rest = target.substr(q + 1);
    size_t at = 0;
    while (at <= rest.size()) {
        size_t amp = rest.find('&', at);
        std::string pair = rest.substr(at, amp == std::string::npos ? std::string::npos : amp - at);
        if (pair.compare(0, key.size() + 1, key + "=") == 0) return pair.substr(key.size() + 1);
        if (amp == std::string::npos) break;
        at = amp + 1;
    }
    return "";
}

void send_all(int fd, const void* data, size_t n) {
    const char* p = static_cast<const char*>(data);
    while (n > 0) {
        ssize_t w = send(fd, p, n, 0);
        if (w <= 0) return;
        p += w;
        n -= static_cast<size_t>(w);
    }
}

void respond(int fd, const char* type, const void* body, size_t n) {
    char head[256];
    int h = snprintf(head, sizeof head,
                     "HTTP/1.1 200 OK\r\nContent-Type: %s\r\nContent-Length: %zu\r\n"
                     "Cache-Control: no-store\r\nConnection: close\r\n\r\n",
                     type, n);
    send_all(fd, head, static_cast<size_t>(h));
    send_all(fd, body, n);
}

void serve_one(int fd) {
    std::string request;
    char buf[4096];
    size_t body_at = std::string::npos;
    size_t length = 0;
    while (true) {
        ssize_t r = recv(fd, buf, sizeof buf, 0);
        if (r <= 0) break;
        request.append(buf, static_cast<size_t>(r));
        if (body_at == std::string::npos) {
            body_at = request.find("\r\n\r\n");
            if (body_at != std::string::npos) {
                body_at += 4;
                auto cl = request.find("Content-Length:");
                if (cl == std::string::npos) cl = request.find("content-length:");
                if (cl != std::string::npos && cl < body_at) length = strtoul(request.c_str() + cl + 15, nullptr, 10);
            }
        }
        if (body_at != std::string::npos && request.size() >= body_at + length) break;
    }
    if (body_at == std::string::npos) return;
    std::string method = request.substr(0, request.find(' '));
    size_t t0 = request.find(' ') + 1;
    std::string target = request.substr(t0, request.find(' ', t0) - t0);
    std::string path = target.substr(0, target.find('?'));
    std::string body = request.substr(body_at, length);

    std::lock_guard<std::mutex> guard(panel.lock);
    if (method == "GET" && path == "/") {
        respond(fd, "text/html; charset=utf-8", panel.page.data(), panel.page.size());
    } else if (method == "GET" && path == "/state") {
        size_t from = strtoul(query(target, "uart").c_str(), nullptr, 10);
        if (from > panel.uart_out.size()) from = panel.uart_out.size();
        std::string json = "{";
        char num[160];
        snprintf(num, sizeof num,
                 "\"rgb\":%u,\"leds\":%u,\"buttons\":%u,\"switches\":%u,\"frames\":%llu,\"fps\":%.2f,"
                 "\"mhz\":%.3f,\"screen\":%s,\"uart_at\":%zu,\"uart\":[",
                 panel.leds[0], panel.leds[1], panel.inputs[0], panel.inputs[1],
                 static_cast<unsigned long long>(panel.frames), panel.fps, panel.cycles_per_second / 1e6,
                 panel.screen.load() ? "true" : "false", panel.uart_out.size());
        json += num;
        for (size_t i = from; i < panel.uart_out.size(); i++) {
            if (i > from) json += ",";
            json += std::to_string(panel.uart_out[i]);
        }
        json += "]}";
        respond(fd, "application/json", json.data(), json.size());
    } else if (method == "GET" && path == "/frame") {
        respond(fd, "application/octet-stream", panel.shown.data(), panel.shown.size() * 2);
    } else if (method == "POST" && path == "/input") {
        std::string b = query(target, "buttons"), s = query(target, "switches");
        if (!b.empty()) panel.inputs[0] = static_cast<uint32_t>(strtoul(b.c_str(), nullptr, 10));
        if (!s.empty()) panel.inputs[1] = static_cast<uint32_t>(strtoul(s.c_str(), nullptr, 10));
        respond(fd, "text/plain", "ok", 2);
    } else if (method == "POST" && path == "/uart") {
        for (char c : body) panel.uart_in.push_back(static_cast<uint8_t>(c));
        respond(fd, "text/plain", "ok", 2);
    } else {
        const char* msg = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        send_all(fd, msg, strlen(msg));
    }
}

void server(int listener) {
    while (true) {
        int fd = accept(listener, nullptr, nullptr);
        if (fd < 0) continue;
        serve_one(fd);
        close(fd);
    }
}

}  // namespace

// Starts the panel: `page` is vboard.html's path; the port goes to
// `port_file`, and to stdout. Returns 0, or -1 if no socket could be had.
extern "C" int vb_open(const char* page, const char* port_file) {
    if (FILE* f = fopen(page, "rb")) {
        char buf[8192];
        size_t n;
        while ((n = fread(buf, 1, sizeof buf, f)) > 0) panel.page.append(buf, n);
        fclose(f);
    } else {
        panel.page = "<p>vboard: cannot read the page</p>";
    }
    int listener = socket(AF_INET, SOCK_STREAM, 0);
    int yes = 1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof yes);
    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = 0;
    if (listener < 0 || bind(listener, reinterpret_cast<sockaddr*>(&addr), sizeof addr) != 0 || listen(listener, 16) != 0) {
        perror("vboard: socket");
        return -1;
    }
    socklen_t len = sizeof addr;
    getsockname(listener, reinterpret_cast<sockaddr*>(&addr), &len);
    int port = ntohs(addr.sin_port);
    if (FILE* f = fopen(port_file, "w")) {
        fprintf(f, "%d\n", port);
        fclose(f);
    }
    printf("vboard: http://127.0.0.1:%d/\n", port);
    fflush(stdout);
    std::thread(server, listener).detach();
    return 0;
}

// An output's new value: 0 the RGB LED, 1 the row of LEDs.
extern "C" void vb_leds(int which, int value) {
    std::lock_guard<std::mutex> guard(panel.lock);
    if (which >= 0 && which < 4) panel.leds[which] = static_cast<uint32_t>(value);
}

// An input's value: 0 the buttons, 1 the switches.
extern "C" int vb_inputs(int which) {
    std::lock_guard<std::mutex> guard(panel.lock);
    return which >= 0 && which < 4 ? static_cast<int>(panel.inputs[which]) : 0;
}

// A byte the design sent on its serial port.
extern "C" void vb_uart_out(int c) {
    std::lock_guard<std::mutex> guard(panel.lock);
    panel.uart_out.push_back(static_cast<uint8_t>(c));
}

// The next byte typed in the page for the design, or -1.
extern "C" int vb_uart_in() {
    std::lock_guard<std::mutex> guard(panel.lock);
    if (panel.uart_in.empty()) return -1;
    int c = panel.uart_in.front();
    panel.uart_in.pop_front();
    return c;
}

// One visible pixel, 12 bits {r, g, b}. Called for every pixel of every
// frame, so it takes no lock: only this thread writes the frame being drawn.
extern "C" void vb_pixel(int x, int y, int rgb) {
    if (x >= 0 && x < WIDTH && y >= 0 && y < HEIGHT) panel.drawing[y * WIDTH + x] = static_cast<uint16_t>(rgb);
    panel.screen = true;
}

// The frame just drawn is whole: show it.
extern "C" void vb_frame() {
    std::lock_guard<std::mutex> guard(panel.lock);
    panel.shown = panel.drawing;
    panel.frames++;
    auto now = std::chrono::steady_clock::now();
    double dt = std::chrono::duration<double>(now - panel.last_frame).count();
    if (dt > 0) panel.fps = panel.fps == 0 ? 1 / dt : 0.8 * panel.fps + 0.2 / dt;
    panel.last_frame = now;
}

// `n` more clock cycles simulated, for the page's speed readout.
extern "C" void vb_cycles(int n) {
    std::lock_guard<std::mutex> guard(panel.lock);
    panel.cycles += static_cast<uint64_t>(n);
    auto now = std::chrono::steady_clock::now();
    double dt = std::chrono::duration<double>(now - panel.last_rate).count();
    if (dt >= 0.5) {
        panel.cycles_per_second = (panel.cycles - panel.cycles_at_rate) / dt;
        panel.cycles_at_rate = panel.cycles;
        panel.last_rate = now;
    }
}
