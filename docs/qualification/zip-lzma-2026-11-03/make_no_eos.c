/* Fixture producer only: independent liblzma LZMA1EXT encoder without EOPM. */
#include <lzma.h>
#include <stdint.h>
#include <stdio.h>
int main(int argc, char **argv) {
    if (argc != 2) return 1;
    FILE *input = fopen(argv[1], "rb");
    if (!input) return 2;
    uint8_t plain[4096], packed[8192], properties[5];
    size_t count = fread(plain, 1, sizeof plain, input);
    if (ferror(input) || fgetc(input) != EOF) return 3;
    fclose(input);
    lzma_options_lzma options;
    if (lzma_lzma_preset(&options, 6)) return 4;
    options.ext_flags = 0;
    lzma_filter filters[] = {{LZMA_FILTER_LZMA1EXT, &options}, {LZMA_VLI_UNKNOWN, NULL}};
    size_t written = 0;
    if (lzma_raw_buffer_encode(filters, NULL, plain, count, packed, &written, sizeof packed) != LZMA_OK) return 5;
    lzma_filter props = {LZMA_FILTER_LZMA1, &options};
    if (lzma_properties_encode(&props, properties) != LZMA_OK) return 6;
    const uint8_t header[] = {9,4,5,0};
    if (fwrite(header,1,4,stdout) != 4 || fwrite(properties,1,5,stdout) != 5 || fwrite(packed,1,written,stdout) != written) return 7;
    return 0;
}
