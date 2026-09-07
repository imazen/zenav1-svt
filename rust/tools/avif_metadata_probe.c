/* Independent libavif reader for animation metadata/association gates.
 * Build against the installed libavif version, outside the source tree. */
#include <avif/avif.h>
#include <stdio.h>
#include <stdlib.h>
#include <inttypes.h>
#include <string.h>
static void bytes(const char *name, const avifRWData *data) {
    printf("%s=", name);
    for (size_t i=0; i<data->size; ++i) printf("%02x", data->data[i]);
    putchar('\n');
}
static int same_pixels(const avifImage *a, const avifImage *b) {
    if (a->width != b->width || a->height != b->height || a->depth != b->depth ||
        a->yuvFormat != b->yuvFormat || a->alphaPremultiplied != b->alphaPremultiplied) return 0;
    for (int c = AVIF_CHAN_Y; c <= AVIF_CHAN_A; ++c) {
        const uint8_t *ap = avifImagePlane(a, c), *bp = avifImagePlane(b, c);
        if (!ap != !bp) return 0;
        if (!ap) continue;
        const uint32_t h = avifImagePlaneHeight(a, c);
        const size_t row = (size_t)avifImagePlaneWidth(a, c) * (a->depth > 8 ? 2 : 1);
        for (uint32_t y = 0; y < h; ++y) {
            if (memcmp(ap + (size_t)y * avifImagePlaneRowBytes(a, c),
                       bp + (size_t)y * avifImagePlaneRowBytes(b, c), row)) return 0;
        }
    }
    return 1;
}

// Called after the first image is decoded. Compare every plane byte after
// reverse and alternating seeks and decoder reset against sequential decoding.
static int timing_and_seek(avifDecoder *d) {
    if (d->imageCount <= 0 || d->imageCount > 4096) return 1;
    const uint32_t count = (uint32_t)d->imageCount;
    avifImage **images = calloc(count, sizeof(*images));
    if (!images) return 1;
    int failed = 1;
    printf("timescale=%" PRIu64 "\nduration=%" PRIu64 "\n", d->timescale, d->durationInTimescales);
    for (uint32_t i = 0; i < count; ++i) {
        if (i && avifDecoderNextImage(d) != AVIF_RESULT_OK) goto done;
        avifImageTiming timing;
        if (avifDecoderNthImageTiming(d, i, &timing) != AVIF_RESULT_OK ||
            timing.timescale != d->imageTiming.timescale ||
            timing.ptsInTimescales != d->imageTiming.ptsInTimescales ||
            timing.durationInTimescales != d->imageTiming.durationInTimescales ||
            !avifDecoderIsKeyframe(d, i) || avifDecoderNearestKeyframe(d, i) != i) goto done;
        printf("frame%u=%" PRIu64 ",%" PRIu64 "\n", i, timing.ptsInTimescales, timing.durationInTimescales);
        images[i] = avifImageCreateEmpty();
        if (!images[i] || avifImageCopy(images[i], d->image, AVIF_PLANES_ALL) != AVIF_RESULT_OK) goto done;
        // Every fixture frame must be distinguishable, even without alpha.
        for (uint32_t previous = 0; previous < i; ++previous) {
            if (same_pixels(images[previous], images[i])) goto done;
        }
    }
    if (avifDecoderNextImage(d) != AVIF_RESULT_NO_IMAGES_REMAINING ||
        avifDecoderNthImage(d, count) != AVIF_RESULT_NO_IMAGES_REMAINING) goto done;
    for (uint32_t pass = 0; pass < 2; ++pass) {
        for (uint32_t i = 0; i < count; ++i) {
            const uint32_t index = pass == 0 ? count-1-i : (i % 2 == 0 ? i/2 : count-1-i/2);
            if (avifDecoderNthImage(d, index) != AVIF_RESULT_OK || !same_pixels(images[index], d->image)) goto done;
            avifImageTiming timing;
            if (avifDecoderNthImageTiming(d, index, &timing) != AVIF_RESULT_OK ||
                timing.ptsInTimescales != d->imageTiming.ptsInTimescales ||
                timing.durationInTimescales != d->imageTiming.durationInTimescales) goto done;
        }
    }
    if (avifDecoderReset(d) != AVIF_RESULT_OK) goto done;
    for (uint32_t i = 0; i < count; ++i) {
        if (avifDecoderNextImage(d) != AVIF_RESULT_OK || !same_pixels(images[i], d->image)) goto done;
    }
    if (avifDecoderNextImage(d) != AVIF_RESULT_NO_IMAGES_REMAINING) goto done;
    printf("seek=exact\n");
    failed = 0;
done:
    for (uint32_t i = 0; i < count; ++i) if (images[i]) avifImageDestroy(images[i]);
    free(images);
    if (failed) fprintf(stderr, "timing/seek comparison failed: %s\n", d->diag.error);
    return failed;
}

int main(int argc, char **argv) {
    if (argc < 2 || argc > 3) return 2;
    avifDecoder *d = avifDecoderCreate();
    if (!d) return 3;
    d->maxThreads = 1;
    avifResult r = AVIF_RESULT_OK;
    const int timing = argc == 3 && !strcmp(argv[2], "timing");
    if (timing) r = avifDecoderSetSource(d, AVIF_DECODER_SOURCE_TRACKS);
    if (argc == 3 && !strcmp(argv[2], "poster")) r = avifDecoderSetSource(d, AVIF_DECODER_SOURCE_PRIMARY_ITEM);
    if (r == AVIF_RESULT_OK) r = avifDecoderSetIOFile(d, argv[1]);
    if (r == AVIF_RESULT_OK) r = avifDecoderParse(d);
    if (r == AVIF_RESULT_OK) r = avifDecoderNextImage(d);
    if (r != AVIF_RESULT_OK) { fprintf(stderr, "%s: %s\n", avifResultToString(r), d->diag.error); avifDecoderDestroy(d); return 1; }
    printf("frames=%d\nrepeat=%d\nalpha=%d\npremultiplied=%d\n", d->imageCount, d->repetitionCount, d->image->alphaPlane != NULL, d->image->alphaPremultiplied);
    printf("cicp=%u,%u,%u,%u\nclli=%u,%u\n", d->image->colorPrimaries, d->image->transferCharacteristics, d->image->matrixCoefficients, d->image->yuvRange, d->image->clli.maxCLL, d->image->clli.maxPALL);
    if (d->image->transformFlags & AVIF_TRANSFORM_PASP) printf("pasp=%u,%u\n", d->image->pasp.hSpacing, d->image->pasp.vSpacing);
    else printf("pasp=none\n");
    if (d->image->transformFlags & AVIF_TRANSFORM_IROT) printf("rotation=%u\n", d->image->irot.angle);
    else printf("rotation=none\n");
    if (d->image->transformFlags & AVIF_TRANSFORM_IMIR) printf("mirror=%u\n", d->image->imir.axis);
    else printf("mirror=none\n");
    if (d->image->transformFlags & AVIF_TRANSFORM_CLAP) {
        avifCropRect crop;
        if (!avifCropRectFromCleanApertureBox(&crop, &d->image->clap, d->image->width, d->image->height, &d->diag)) {
            fprintf(stderr, "invalid crop: %s\n", d->diag.error); avifDecoderDestroy(d); return 1;
        }
        printf("crop=%u,%u,%u,%u\n", crop.x, crop.y, crop.width, crop.height);
    } else printf("crop=none\n");
    printf("monochrome=%d\n", d->image->yuvFormat == AVIF_PIXEL_FORMAT_YUV400);
    printf("dimensions=%u,%u\n", d->image->width, d->image->height);
    bytes("icc", &d->image->icc); bytes("exif", &d->image->exif); bytes("xmp", &d->image->xmp);
    if (timing) { const int failed = timing_and_seek(d); avifDecoderDestroy(d); return failed; }
    while ((r = avifDecoderNextImage(d)) == AVIF_RESULT_OK) {}
    avifDecoderDestroy(d);
    return r == AVIF_RESULT_NO_IMAGES_REMAINING ? 0 : 1;
}
