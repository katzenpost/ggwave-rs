// Test tool: C ggwave encoder/decoder over raw little-endian samples on stdin/stdout.
// Built by tools/build-c-ggwave.sh against C ggwave at the pinned commit. Not part of the crate.
//
//   ggwave-c encode -p <protocol id> -v <volume> [-l <fixed len>] [-r <rate out>] [-d] [-s] <payload hex>
//   ggwave-c decode [-l <fixed len>] [-r <rate in>] [-d] [-s]  < samples
//
// -s selects i16 samples (default f32), -d turns on DSS. decode prints each payload as hex on its own line.

#include "ggwave/ggwave.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

int main(int argc, char ** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: ggwave-c encode|decode [options]\n");
        return 2;
    }
    const std::string cmd = argv[1];
    int protocol = 1, volume = 50, fixedLen = -1, dss = 0, i16 = 0;
    float rate = 48000.0f;
    std::string hex;
    for (int i = 2; i < argc; ++i) {
        const std::string a = argv[i];
        if (a == "-p" && i + 1 < argc) protocol = atoi(argv[++i]);
        else if (a == "-v" && i + 1 < argc) volume = atoi(argv[++i]);
        else if (a == "-l" && i + 1 < argc) fixedLen = atoi(argv[++i]);
        else if (a == "-r" && i + 1 < argc) rate = atof(argv[++i]);
        else if (a == "-d") dss = 1;
        else if (a == "-s") i16 = 1;
        else hex = a;
    }

    GGWave::setLogFile(nullptr);
    auto params = GGWave::getDefaultParameters();
    params.payloadLength = fixedLen;
    params.sampleFormatInp = params.sampleFormatOut = i16 ? GGWAVE_SAMPLE_FORMAT_I16 : GGWAVE_SAMPLE_FORMAT_F32;
    if (cmd == "encode") {
        params.sampleRateOut = rate;
        params.operatingMode = GGWAVE_OPERATING_MODE_TX;
    } else {
        params.sampleRateInp = rate;
        params.operatingMode = GGWAVE_OPERATING_MODE_RX;
    }
    if (dss) params.operatingMode |= GGWAVE_OPERATING_MODE_USE_DSS;

    GGWave gg(params);

    if (cmd == "encode") {
        std::vector<char> payload;
        for (size_t i = 0; i + 1 < hex.size(); i += 2) {
            const int hi = hexval(hex[i]), lo = hexval(hex[i + 1]);
            if (hi < 0 || lo < 0) { fprintf(stderr, "bad hex\n"); return 2; }
            payload.push_back((char) (hi * 16 + lo));
        }
        if (!gg.init((int) payload.size(), payload.data(), (GGWave::TxProtocolId) protocol, volume)) return 1;
        const uint32_t n = gg.encode();
        if (n == 0) return 1;
        fwrite(gg.txWaveform(), 1, n, stdout);
        return 0;
    }

    if (cmd == "decode") {
        std::vector<char> buf(4096 * (i16 ? 2 : 4));
        GGWave::TxRxData data;
        size_t n;
        while ((n = fread(buf.data(), 1, buf.size(), stdin)) > 0) {
            gg.decode(buf.data(), (uint32_t) n);
            const int len = gg.rxTakeData(data);
            if (len > 0) {
                for (int i = 0; i < len; ++i) printf("%02x", (uint8_t) data[i]);
                printf("\n");
            }
        }
        return 0;
    }

    fprintf(stderr, "unknown command %s\n", cmd.c_str());
    return 2;
}
