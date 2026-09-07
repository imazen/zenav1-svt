/* Independent libavif reader for animation metadata/association gates.
 * Build against the installed libavif version, outside the source tree. */
#include <avif/avif.h>
#include <stdio.h>
#include <string.h>
static void bytes(const char *name, const avifRWData *data) {
    printf("%s=", name);
    for (size_t i=0; i<data->size; ++i) printf("%02x", data->data[i]);
    putchar('\n');
}
int main(int argc, char **argv) {
    if (argc < 2 || argc > 3) return 2;
    avifDecoder *d = avifDecoderCreate();
    if (!d) return 3;
    d->maxThreads = 1;
    avifResult r = AVIF_RESULT_OK;
    if (argc == 3 && !strcmp(argv[2], "poster")) r = avifDecoderSetSource(d, AVIF_DECODER_SOURCE_PRIMARY_ITEM);
    if (r == AVIF_RESULT_OK) r = avifDecoderSetIOFile(d, argv[1]);
    if (r == AVIF_RESULT_OK) r = avifDecoderParse(d);
    if (r == AVIF_RESULT_OK) r = avifDecoderNextImage(d);
    if (r != AVIF_RESULT_OK) { fprintf(stderr, "%s: %s\n", avifResultToString(r), d->diag.error); avifDecoderDestroy(d); return 1; }
    printf("frames=%d\nrepeat=%d\nalpha=%d\npremultiplied=%d\n", d->imageCount, d->repetitionCount, d->image->alphaPlane != NULL, d->image->alphaPremultiplied);
    printf("cicp=%u,%u,%u,%u\nclli=%u,%u\n", d->image->colorPrimaries, d->image->transferCharacteristics, d->image->matrixCoefficients, d->image->yuvRange, d->image->clli.maxCLL, d->image->clli.maxPALL);
    bytes("icc", &d->image->icc); bytes("exif", &d->image->exif); bytes("xmp", &d->image->xmp);
    while ((r = avifDecoderNextImage(d)) == AVIF_RESULT_OK) {}
    avifDecoderDestroy(d);
    return r == AVIF_RESULT_NO_IMAGES_REMAINING ? 0 : 1;
}
