use super::*;
#[rite]
pub(super) fn fdct64_x8(t: Desktop64, inp: &[__m256i; 64], out: &mut [__m256i; 64], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1
    let s1: [__m256i; 64] = [
        add!(inp[0], inp[63]), add!(inp[1], inp[62]), add!(inp[2], inp[61]), add!(inp[3], inp[60]),
        add!(inp[4], inp[59]), add!(inp[5], inp[58]), add!(inp[6], inp[57]), add!(inp[7], inp[56]),
        add!(inp[8], inp[55]), add!(inp[9], inp[54]), add!(inp[10], inp[53]), add!(inp[11], inp[52]),
        add!(inp[12], inp[51]), add!(inp[13], inp[50]), add!(inp[14], inp[49]), add!(inp[15], inp[48]),
        add!(inp[16], inp[47]), add!(inp[17], inp[46]), add!(inp[18], inp[45]), add!(inp[19], inp[44]),
        add!(inp[20], inp[43]), add!(inp[21], inp[42]), add!(inp[22], inp[41]), add!(inp[23], inp[40]),
        add!(inp[24], inp[39]), add!(inp[25], inp[38]), add!(inp[26], inp[37]), add!(inp[27], inp[36]),
        add!(inp[28], inp[35]), add!(inp[29], inp[34]), add!(inp[30], inp[33]), add!(inp[31], inp[32]),
        sub!(inp[31], inp[32]), sub!(inp[30], inp[33]), sub!(inp[29], inp[34]), sub!(inp[28], inp[35]),
        sub!(inp[27], inp[36]), sub!(inp[26], inp[37]), sub!(inp[25], inp[38]), sub!(inp[24], inp[39]),
        sub!(inp[23], inp[40]), sub!(inp[22], inp[41]), sub!(inp[21], inp[42]), sub!(inp[20], inp[43]),
        sub!(inp[19], inp[44]), sub!(inp[18], inp[45]), sub!(inp[17], inp[46]), sub!(inp[16], inp[47]),
        sub!(inp[15], inp[48]), sub!(inp[14], inp[49]), sub!(inp[13], inp[50]), sub!(inp[12], inp[51]),
        sub!(inp[11], inp[52]), sub!(inp[10], inp[53]), sub!(inp[9], inp[54]), sub!(inp[8], inp[55]),
        sub!(inp[7], inp[56]), sub!(inp[6], inp[57]), sub!(inp[5], inp[58]), sub!(inp[4], inp[59]),
        sub!(inp[3], inp[60]), sub!(inp[2], inp[61]), sub!(inp[1], inp[62]), sub!(inp[0], inp[63]),
    ];
    // stage 2
    let s2: [__m256i; 64] = [
        add!(s1[0], s1[31]), add!(s1[1], s1[30]), add!(s1[2], s1[29]), add!(s1[3], s1[28]),
        add!(s1[4], s1[27]), add!(s1[5], s1[26]), add!(s1[6], s1[25]), add!(s1[7], s1[24]),
        add!(s1[8], s1[23]), add!(s1[9], s1[22]), add!(s1[10], s1[21]), add!(s1[11], s1[20]),
        add!(s1[12], s1[19]), add!(s1[13], s1[18]), add!(s1[14], s1[17]), add!(s1[15], s1[16]),
        sub!(s1[15], s1[16]), sub!(s1[14], s1[17]), sub!(s1[13], s1[18]), sub!(s1[12], s1[19]),
        sub!(s1[11], s1[20]), sub!(s1[10], s1[21]), sub!(s1[9], s1[22]), sub!(s1[8], s1[23]),
        sub!(s1[7], s1[24]), sub!(s1[6], s1[25]), sub!(s1[5], s1[26]), sub!(s1[4], s1[27]),
        sub!(s1[3], s1[28]), sub!(s1[2], s1[29]), sub!(s1[1], s1[30]), sub!(s1[0], s1[31]),
        s1[32], s1[33], s1[34], s1[35],
        s1[36], s1[37], s1[38], s1[39],
        hbtf(t, cn!(t, cospi, 32), s1[40], c!(t, cospi, 32), s1[55], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[41], c!(t, cospi, 32), s1[54], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[42], c!(t, cospi, 32), s1[53], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[43], c!(t, cospi, 32), s1[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 32), s1[44], c!(t, cospi, 32), s1[51], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[45], c!(t, cospi, 32), s1[50], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[46], c!(t, cospi, 32), s1[49], rnd, sh), hbtf(t, cn!(t, cospi, 32), s1[47], c!(t, cospi, 32), s1[48], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[48], c!(t, cospi, 32), s1[47], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[49], c!(t, cospi, 32), s1[46], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[50], c!(t, cospi, 32), s1[45], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[51], c!(t, cospi, 32), s1[44], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s1[52], c!(t, cospi, 32), s1[43], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[53], c!(t, cospi, 32), s1[42], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[54], c!(t, cospi, 32), s1[41], rnd, sh), hbtf(t, c!(t, cospi, 32), s1[55], c!(t, cospi, 32), s1[40], rnd, sh),
        s1[56], s1[57], s1[58], s1[59],
        s1[60], s1[61], s1[62], s1[63],
    ];
    // stage 3
    let s3: [__m256i; 64] = [
        add!(s2[0], s2[15]), add!(s2[1], s2[14]), add!(s2[2], s2[13]), add!(s2[3], s2[12]),
        add!(s2[4], s2[11]), add!(s2[5], s2[10]), add!(s2[6], s2[9]), add!(s2[7], s2[8]),
        sub!(s2[7], s2[8]), sub!(s2[6], s2[9]), sub!(s2[5], s2[10]), sub!(s2[4], s2[11]),
        sub!(s2[3], s2[12]), sub!(s2[2], s2[13]), sub!(s2[1], s2[14]), sub!(s2[0], s2[15]),
        s2[16], s2[17], s2[18], s2[19],
        hbtf(t, cn!(t, cospi, 32), s2[20], c!(t, cospi, 32), s2[27], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[21], c!(t, cospi, 32), s2[26], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[22], c!(t, cospi, 32), s2[25], rnd, sh), hbtf(t, cn!(t, cospi, 32), s2[23], c!(t, cospi, 32), s2[24], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s2[24], c!(t, cospi, 32), s2[23], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[25], c!(t, cospi, 32), s2[22], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[26], c!(t, cospi, 32), s2[21], rnd, sh), hbtf(t, c!(t, cospi, 32), s2[27], c!(t, cospi, 32), s2[20], rnd, sh),
        s2[28], s2[29], s2[30], s2[31],
        add!(s2[32], s2[47]), add!(s2[33], s2[46]), add!(s2[34], s2[45]), add!(s2[35], s2[44]),
        add!(s2[36], s2[43]), add!(s2[37], s2[42]), add!(s2[38], s2[41]), add!(s2[39], s2[40]),
        sub!(s2[39], s2[40]), sub!(s2[38], s2[41]), sub!(s2[37], s2[42]), sub!(s2[36], s2[43]),
        sub!(s2[35], s2[44]), sub!(s2[34], s2[45]), sub!(s2[33], s2[46]), sub!(s2[32], s2[47]),
        sub!(s2[63], s2[48]), sub!(s2[62], s2[49]), sub!(s2[61], s2[50]), sub!(s2[60], s2[51]),
        sub!(s2[59], s2[52]), sub!(s2[58], s2[53]), sub!(s2[57], s2[54]), sub!(s2[56], s2[55]),
        add!(s2[56], s2[55]), add!(s2[57], s2[54]), add!(s2[58], s2[53]), add!(s2[59], s2[52]),
        add!(s2[60], s2[51]), add!(s2[61], s2[50]), add!(s2[62], s2[49]), add!(s2[63], s2[48]),
    ];
    // stage 4
    let s4: [__m256i; 64] = [
        add!(s3[0], s3[7]), add!(s3[1], s3[6]), add!(s3[2], s3[5]), add!(s3[3], s3[4]),
        sub!(s3[3], s3[4]), sub!(s3[2], s3[5]), sub!(s3[1], s3[6]), sub!(s3[0], s3[7]),
        s3[8], s3[9], hbtf(t, cn!(t, cospi, 32), s3[10], c!(t, cospi, 32), s3[13], rnd, sh), hbtf(t, cn!(t, cospi, 32), s3[11], c!(t, cospi, 32), s3[12], rnd, sh),
        hbtf(t, c!(t, cospi, 32), s3[12], c!(t, cospi, 32), s3[11], rnd, sh), hbtf(t, c!(t, cospi, 32), s3[13], c!(t, cospi, 32), s3[10], rnd, sh), s3[14], s3[15],
        add!(s3[16], s3[23]), add!(s3[17], s3[22]), add!(s3[18], s3[21]), add!(s3[19], s3[20]),
        sub!(s3[19], s3[20]), sub!(s3[18], s3[21]), sub!(s3[17], s3[22]), sub!(s3[16], s3[23]),
        sub!(s3[31], s3[24]), sub!(s3[30], s3[25]), sub!(s3[29], s3[26]), sub!(s3[28], s3[27]),
        add!(s3[28], s3[27]), add!(s3[29], s3[26]), add!(s3[30], s3[25]), add!(s3[31], s3[24]),
        s3[32], s3[33], s3[34], s3[35],
        hbtf(t, cn!(t, cospi, 16), s3[36], c!(t, cospi, 48), s3[59], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[37], c!(t, cospi, 48), s3[58], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[38], c!(t, cospi, 48), s3[57], rnd, sh), hbtf(t, cn!(t, cospi, 16), s3[39], c!(t, cospi, 48), s3[56], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s3[40], cn!(t, cospi, 16), s3[55], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[41], cn!(t, cospi, 16), s3[54], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[42], cn!(t, cospi, 16), s3[53], rnd, sh), hbtf(t, cn!(t, cospi, 48), s3[43], cn!(t, cospi, 16), s3[52], rnd, sh),
        s3[44], s3[45], s3[46], s3[47],
        s3[48], s3[49], s3[50], s3[51],
        hbtf(t, c!(t, cospi, 48), s3[52], cn!(t, cospi, 16), s3[43], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[53], cn!(t, cospi, 16), s3[42], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[54], cn!(t, cospi, 16), s3[41], rnd, sh), hbtf(t, c!(t, cospi, 48), s3[55], cn!(t, cospi, 16), s3[40], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s3[56], c!(t, cospi, 48), s3[39], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[57], c!(t, cospi, 48), s3[38], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[58], c!(t, cospi, 48), s3[37], rnd, sh), hbtf(t, c!(t, cospi, 16), s3[59], c!(t, cospi, 48), s3[36], rnd, sh),
        s3[60], s3[61], s3[62], s3[63],
    ];
    // stage 5
    let s5: [__m256i; 64] = [
        add!(s4[0], s4[3]), add!(s4[1], s4[2]), sub!(s4[1], s4[2]), sub!(s4[0], s4[3]),
        s4[4], hbtf(t, cn!(t, cospi, 32), s4[5], c!(t, cospi, 32), s4[6], rnd, sh), hbtf(t, c!(t, cospi, 32), s4[6], c!(t, cospi, 32), s4[5], rnd, sh), s4[7],
        add!(s4[8], s4[11]), add!(s4[9], s4[10]), sub!(s4[9], s4[10]), sub!(s4[8], s4[11]),
        sub!(s4[15], s4[12]), sub!(s4[14], s4[13]), add!(s4[14], s4[13]), add!(s4[15], s4[12]),
        s4[16], s4[17], hbtf(t, cn!(t, cospi, 16), s4[18], c!(t, cospi, 48), s4[29], rnd, sh), hbtf(t, cn!(t, cospi, 16), s4[19], c!(t, cospi, 48), s4[28], rnd, sh),
        hbtf(t, cn!(t, cospi, 48), s4[20], cn!(t, cospi, 16), s4[27], rnd, sh), hbtf(t, cn!(t, cospi, 48), s4[21], cn!(t, cospi, 16), s4[26], rnd, sh), s4[22], s4[23],
        s4[24], s4[25], hbtf(t, c!(t, cospi, 48), s4[26], cn!(t, cospi, 16), s4[21], rnd, sh), hbtf(t, c!(t, cospi, 48), s4[27], cn!(t, cospi, 16), s4[20], rnd, sh),
        hbtf(t, c!(t, cospi, 16), s4[28], c!(t, cospi, 48), s4[19], rnd, sh), hbtf(t, c!(t, cospi, 16), s4[29], c!(t, cospi, 48), s4[18], rnd, sh), s4[30], s4[31],
        add!(s4[32], s4[39]), add!(s4[33], s4[38]), add!(s4[34], s4[37]), add!(s4[35], s4[36]),
        sub!(s4[35], s4[36]), sub!(s4[34], s4[37]), sub!(s4[33], s4[38]), sub!(s4[32], s4[39]),
        sub!(s4[47], s4[40]), sub!(s4[46], s4[41]), sub!(s4[45], s4[42]), sub!(s4[44], s4[43]),
        add!(s4[44], s4[43]), add!(s4[45], s4[42]), add!(s4[46], s4[41]), add!(s4[47], s4[40]),
        add!(s4[48], s4[55]), add!(s4[49], s4[54]), add!(s4[50], s4[53]), add!(s4[51], s4[52]),
        sub!(s4[51], s4[52]), sub!(s4[50], s4[53]), sub!(s4[49], s4[54]), sub!(s4[48], s4[55]),
        sub!(s4[63], s4[56]), sub!(s4[62], s4[57]), sub!(s4[61], s4[58]), sub!(s4[60], s4[59]),
        add!(s4[60], s4[59]), add!(s4[61], s4[58]), add!(s4[62], s4[57]), add!(s4[63], s4[56]),
    ];
    // stage 6
    let s6: [__m256i; 64] = [
        hbtf(t, c!(t, cospi, 32), s5[0], c!(t, cospi, 32), s5[1], rnd, sh), hbtf(t, cn!(t, cospi, 32), s5[1], c!(t, cospi, 32), s5[0], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[2], c!(t, cospi, 16), s5[3], rnd, sh), hbtf(t, c!(t, cospi, 48), s5[3], cn!(t, cospi, 16), s5[2], rnd, sh),
        add!(s5[4], s5[5]), sub!(s5[4], s5[5]), sub!(s5[7], s5[6]), add!(s5[7], s5[6]),
        s5[8], hbtf(t, cn!(t, cospi, 16), s5[9], c!(t, cospi, 48), s5[14], rnd, sh), hbtf(t, cn!(t, cospi, 48), s5[10], cn!(t, cospi, 16), s5[13], rnd, sh), s5[11],
        s5[12], hbtf(t, c!(t, cospi, 48), s5[13], cn!(t, cospi, 16), s5[10], rnd, sh), hbtf(t, c!(t, cospi, 16), s5[14], c!(t, cospi, 48), s5[9], rnd, sh), s5[15],
        add!(s5[16], s5[19]), add!(s5[17], s5[18]), sub!(s5[17], s5[18]), sub!(s5[16], s5[19]),
        sub!(s5[23], s5[20]), sub!(s5[22], s5[21]), add!(s5[22], s5[21]), add!(s5[23], s5[20]),
        add!(s5[24], s5[27]), add!(s5[25], s5[26]), sub!(s5[25], s5[26]), sub!(s5[24], s5[27]),
        sub!(s5[31], s5[28]), sub!(s5[30], s5[29]), add!(s5[30], s5[29]), add!(s5[31], s5[28]),
        s5[32], s5[33], hbtf(t, cn!(t, cospi, 8), s5[34], c!(t, cospi, 56), s5[61], rnd, sh), hbtf(t, cn!(t, cospi, 8), s5[35], c!(t, cospi, 56), s5[60], rnd, sh),
        hbtf(t, cn!(t, cospi, 56), s5[36], cn!(t, cospi, 8), s5[59], rnd, sh), hbtf(t, cn!(t, cospi, 56), s5[37], cn!(t, cospi, 8), s5[58], rnd, sh), s5[38], s5[39],
        s5[40], s5[41], hbtf(t, cn!(t, cospi, 40), s5[42], c!(t, cospi, 24), s5[53], rnd, sh), hbtf(t, cn!(t, cospi, 40), s5[43], c!(t, cospi, 24), s5[52], rnd, sh),
        hbtf(t, cn!(t, cospi, 24), s5[44], cn!(t, cospi, 40), s5[51], rnd, sh), hbtf(t, cn!(t, cospi, 24), s5[45], cn!(t, cospi, 40), s5[50], rnd, sh), s5[46], s5[47],
        s5[48], s5[49], hbtf(t, c!(t, cospi, 24), s5[50], cn!(t, cospi, 40), s5[45], rnd, sh), hbtf(t, c!(t, cospi, 24), s5[51], cn!(t, cospi, 40), s5[44], rnd, sh),
        hbtf(t, c!(t, cospi, 40), s5[52], c!(t, cospi, 24), s5[43], rnd, sh), hbtf(t, c!(t, cospi, 40), s5[53], c!(t, cospi, 24), s5[42], rnd, sh), s5[54], s5[55],
        s5[56], s5[57], hbtf(t, c!(t, cospi, 56), s5[58], cn!(t, cospi, 8), s5[37], rnd, sh), hbtf(t, c!(t, cospi, 56), s5[59], cn!(t, cospi, 8), s5[36], rnd, sh),
        hbtf(t, c!(t, cospi, 8), s5[60], c!(t, cospi, 56), s5[35], rnd, sh), hbtf(t, c!(t, cospi, 8), s5[61], c!(t, cospi, 56), s5[34], rnd, sh), s5[62], s5[63],
    ];
    // stage 7
    let s7: [__m256i; 64] = [
        s6[0], s6[1], s6[2], s6[3],
        hbtf(t, c!(t, cospi, 56), s6[4], c!(t, cospi, 8), s6[7], rnd, sh), hbtf(t, c!(t, cospi, 24), s6[5], c!(t, cospi, 40), s6[6], rnd, sh), hbtf(t, c!(t, cospi, 24), s6[6], cn!(t, cospi, 40), s6[5], rnd, sh), hbtf(t, c!(t, cospi, 56), s6[7], cn!(t, cospi, 8), s6[4], rnd, sh),
        add!(s6[8], s6[9]), sub!(s6[8], s6[9]), sub!(s6[11], s6[10]), add!(s6[11], s6[10]),
        add!(s6[12], s6[13]), sub!(s6[12], s6[13]), sub!(s6[15], s6[14]), add!(s6[15], s6[14]),
        s6[16], hbtf(t, cn!(t, cospi, 8), s6[17], c!(t, cospi, 56), s6[30], rnd, sh), hbtf(t, cn!(t, cospi, 56), s6[18], cn!(t, cospi, 8), s6[29], rnd, sh), s6[19],
        s6[20], hbtf(t, cn!(t, cospi, 40), s6[21], c!(t, cospi, 24), s6[26], rnd, sh), hbtf(t, cn!(t, cospi, 24), s6[22], cn!(t, cospi, 40), s6[25], rnd, sh), s6[23],
        s6[24], hbtf(t, c!(t, cospi, 24), s6[25], cn!(t, cospi, 40), s6[22], rnd, sh), hbtf(t, c!(t, cospi, 40), s6[26], c!(t, cospi, 24), s6[21], rnd, sh), s6[27],
        s6[28], hbtf(t, c!(t, cospi, 56), s6[29], cn!(t, cospi, 8), s6[18], rnd, sh), hbtf(t, c!(t, cospi, 8), s6[30], c!(t, cospi, 56), s6[17], rnd, sh), s6[31],
        add!(s6[32], s6[35]), add!(s6[33], s6[34]), sub!(s6[33], s6[34]), sub!(s6[32], s6[35]),
        sub!(s6[39], s6[36]), sub!(s6[38], s6[37]), add!(s6[38], s6[37]), add!(s6[39], s6[36]),
        add!(s6[40], s6[43]), add!(s6[41], s6[42]), sub!(s6[41], s6[42]), sub!(s6[40], s6[43]),
        sub!(s6[47], s6[44]), sub!(s6[46], s6[45]), add!(s6[46], s6[45]), add!(s6[47], s6[44]),
        add!(s6[48], s6[51]), add!(s6[49], s6[50]), sub!(s6[49], s6[50]), sub!(s6[48], s6[51]),
        sub!(s6[55], s6[52]), sub!(s6[54], s6[53]), add!(s6[54], s6[53]), add!(s6[55], s6[52]),
        add!(s6[56], s6[59]), add!(s6[57], s6[58]), sub!(s6[57], s6[58]), sub!(s6[56], s6[59]),
        sub!(s6[63], s6[60]), sub!(s6[62], s6[61]), add!(s6[62], s6[61]), add!(s6[63], s6[60]),
    ];
    // stage 8
    let s8: [__m256i; 64] = [
        s7[0], s7[1], s7[2], s7[3],
        s7[4], s7[5], s7[6], s7[7],
        hbtf(t, c!(t, cospi, 60), s7[8], c!(t, cospi, 4), s7[15], rnd, sh), hbtf(t, c!(t, cospi, 28), s7[9], c!(t, cospi, 36), s7[14], rnd, sh), hbtf(t, c!(t, cospi, 44), s7[10], c!(t, cospi, 20), s7[13], rnd, sh), hbtf(t, c!(t, cospi, 12), s7[11], c!(t, cospi, 52), s7[12], rnd, sh),
        hbtf(t, c!(t, cospi, 12), s7[12], cn!(t, cospi, 52), s7[11], rnd, sh), hbtf(t, c!(t, cospi, 44), s7[13], cn!(t, cospi, 20), s7[10], rnd, sh), hbtf(t, c!(t, cospi, 28), s7[14], cn!(t, cospi, 36), s7[9], rnd, sh), hbtf(t, c!(t, cospi, 60), s7[15], cn!(t, cospi, 4), s7[8], rnd, sh),
        add!(s7[16], s7[17]), sub!(s7[16], s7[17]), sub!(s7[19], s7[18]), add!(s7[19], s7[18]),
        add!(s7[20], s7[21]), sub!(s7[20], s7[21]), sub!(s7[23], s7[22]), add!(s7[23], s7[22]),
        add!(s7[24], s7[25]), sub!(s7[24], s7[25]), sub!(s7[27], s7[26]), add!(s7[27], s7[26]),
        add!(s7[28], s7[29]), sub!(s7[28], s7[29]), sub!(s7[31], s7[30]), add!(s7[31], s7[30]),
        s7[32], hbtf(t, cn!(t, cospi, 4), s7[33], c!(t, cospi, 60), s7[62], rnd, sh), hbtf(t, cn!(t, cospi, 60), s7[34], cn!(t, cospi, 4), s7[61], rnd, sh), s7[35],
        s7[36], hbtf(t, cn!(t, cospi, 36), s7[37], c!(t, cospi, 28), s7[58], rnd, sh), hbtf(t, cn!(t, cospi, 28), s7[38], cn!(t, cospi, 36), s7[57], rnd, sh), s7[39],
        s7[40], hbtf(t, cn!(t, cospi, 20), s7[41], c!(t, cospi, 44), s7[54], rnd, sh), hbtf(t, cn!(t, cospi, 44), s7[42], cn!(t, cospi, 20), s7[53], rnd, sh), s7[43],
        s7[44], hbtf(t, cn!(t, cospi, 52), s7[45], c!(t, cospi, 12), s7[50], rnd, sh), hbtf(t, cn!(t, cospi, 12), s7[46], cn!(t, cospi, 52), s7[49], rnd, sh), s7[47],
        s7[48], hbtf(t, c!(t, cospi, 12), s7[49], cn!(t, cospi, 52), s7[46], rnd, sh), hbtf(t, c!(t, cospi, 52), s7[50], c!(t, cospi, 12), s7[45], rnd, sh), s7[51],
        s7[52], hbtf(t, c!(t, cospi, 44), s7[53], cn!(t, cospi, 20), s7[42], rnd, sh), hbtf(t, c!(t, cospi, 20), s7[54], c!(t, cospi, 44), s7[41], rnd, sh), s7[55],
        s7[56], hbtf(t, c!(t, cospi, 28), s7[57], cn!(t, cospi, 36), s7[38], rnd, sh), hbtf(t, c!(t, cospi, 36), s7[58], c!(t, cospi, 28), s7[37], rnd, sh), s7[59],
        s7[60], hbtf(t, c!(t, cospi, 60), s7[61], cn!(t, cospi, 4), s7[34], rnd, sh), hbtf(t, c!(t, cospi, 4), s7[62], c!(t, cospi, 60), s7[33], rnd, sh), s7[63],
    ];
    // stage 9
    let s9: [__m256i; 64] = [
        s8[0], s8[1], s8[2], s8[3],
        s8[4], s8[5], s8[6], s8[7],
        s8[8], s8[9], s8[10], s8[11],
        s8[12], s8[13], s8[14], s8[15],
        hbtf(t, c!(t, cospi, 62), s8[16], c!(t, cospi, 2), s8[31], rnd, sh), hbtf(t, c!(t, cospi, 30), s8[17], c!(t, cospi, 34), s8[30], rnd, sh), hbtf(t, c!(t, cospi, 46), s8[18], c!(t, cospi, 18), s8[29], rnd, sh), hbtf(t, c!(t, cospi, 14), s8[19], c!(t, cospi, 50), s8[28], rnd, sh),
        hbtf(t, c!(t, cospi, 54), s8[20], c!(t, cospi, 10), s8[27], rnd, sh), hbtf(t, c!(t, cospi, 22), s8[21], c!(t, cospi, 42), s8[26], rnd, sh), hbtf(t, c!(t, cospi, 38), s8[22], c!(t, cospi, 26), s8[25], rnd, sh), hbtf(t, c!(t, cospi, 6), s8[23], c!(t, cospi, 58), s8[24], rnd, sh),
        hbtf(t, c!(t, cospi, 6), s8[24], cn!(t, cospi, 58), s8[23], rnd, sh), hbtf(t, c!(t, cospi, 38), s8[25], cn!(t, cospi, 26), s8[22], rnd, sh), hbtf(t, c!(t, cospi, 22), s8[26], cn!(t, cospi, 42), s8[21], rnd, sh), hbtf(t, c!(t, cospi, 54), s8[27], cn!(t, cospi, 10), s8[20], rnd, sh),
        hbtf(t, c!(t, cospi, 14), s8[28], cn!(t, cospi, 50), s8[19], rnd, sh), hbtf(t, c!(t, cospi, 46), s8[29], cn!(t, cospi, 18), s8[18], rnd, sh), hbtf(t, c!(t, cospi, 30), s8[30], cn!(t, cospi, 34), s8[17], rnd, sh), hbtf(t, c!(t, cospi, 62), s8[31], cn!(t, cospi, 2), s8[16], rnd, sh),
        add!(s8[32], s8[33]), sub!(s8[32], s8[33]), sub!(s8[35], s8[34]), add!(s8[35], s8[34]),
        add!(s8[36], s8[37]), sub!(s8[36], s8[37]), sub!(s8[39], s8[38]), add!(s8[39], s8[38]),
        add!(s8[40], s8[41]), sub!(s8[40], s8[41]), sub!(s8[43], s8[42]), add!(s8[43], s8[42]),
        add!(s8[44], s8[45]), sub!(s8[44], s8[45]), sub!(s8[47], s8[46]), add!(s8[47], s8[46]),
        add!(s8[48], s8[49]), sub!(s8[48], s8[49]), sub!(s8[51], s8[50]), add!(s8[51], s8[50]),
        add!(s8[52], s8[53]), sub!(s8[52], s8[53]), sub!(s8[55], s8[54]), add!(s8[55], s8[54]),
        add!(s8[56], s8[57]), sub!(s8[56], s8[57]), sub!(s8[59], s8[58]), add!(s8[59], s8[58]),
        add!(s8[60], s8[61]), sub!(s8[60], s8[61]), sub!(s8[63], s8[62]), add!(s8[63], s8[62]),
    ];
    // stage 10
    let s10: [__m256i; 64] = [
        s9[0], s9[1], s9[2], s9[3],
        s9[4], s9[5], s9[6], s9[7],
        s9[8], s9[9], s9[10], s9[11],
        s9[12], s9[13], s9[14], s9[15],
        s9[16], s9[17], s9[18], s9[19],
        s9[20], s9[21], s9[22], s9[23],
        s9[24], s9[25], s9[26], s9[27],
        s9[28], s9[29], s9[30], s9[31],
        hbtf(t, c!(t, cospi, 63), s9[32], c!(t, cospi, 1), s9[63], rnd, sh), hbtf(t, c!(t, cospi, 31), s9[33], c!(t, cospi, 33), s9[62], rnd, sh), hbtf(t, c!(t, cospi, 47), s9[34], c!(t, cospi, 17), s9[61], rnd, sh), hbtf(t, c!(t, cospi, 15), s9[35], c!(t, cospi, 49), s9[60], rnd, sh),
        hbtf(t, c!(t, cospi, 55), s9[36], c!(t, cospi, 9), s9[59], rnd, sh), hbtf(t, c!(t, cospi, 23), s9[37], c!(t, cospi, 41), s9[58], rnd, sh), hbtf(t, c!(t, cospi, 39), s9[38], c!(t, cospi, 25), s9[57], rnd, sh), hbtf(t, c!(t, cospi, 7), s9[39], c!(t, cospi, 57), s9[56], rnd, sh),
        hbtf(t, c!(t, cospi, 59), s9[40], c!(t, cospi, 5), s9[55], rnd, sh), hbtf(t, c!(t, cospi, 27), s9[41], c!(t, cospi, 37), s9[54], rnd, sh), hbtf(t, c!(t, cospi, 43), s9[42], c!(t, cospi, 21), s9[53], rnd, sh), hbtf(t, c!(t, cospi, 11), s9[43], c!(t, cospi, 53), s9[52], rnd, sh),
        hbtf(t, c!(t, cospi, 51), s9[44], c!(t, cospi, 13), s9[51], rnd, sh), hbtf(t, c!(t, cospi, 19), s9[45], c!(t, cospi, 45), s9[50], rnd, sh), hbtf(t, c!(t, cospi, 35), s9[46], c!(t, cospi, 29), s9[49], rnd, sh), hbtf(t, c!(t, cospi, 3), s9[47], c!(t, cospi, 61), s9[48], rnd, sh),
        hbtf(t, c!(t, cospi, 3), s9[48], cn!(t, cospi, 61), s9[47], rnd, sh), hbtf(t, c!(t, cospi, 35), s9[49], cn!(t, cospi, 29), s9[46], rnd, sh), hbtf(t, c!(t, cospi, 19), s9[50], cn!(t, cospi, 45), s9[45], rnd, sh), hbtf(t, c!(t, cospi, 51), s9[51], cn!(t, cospi, 13), s9[44], rnd, sh),
        hbtf(t, c!(t, cospi, 11), s9[52], cn!(t, cospi, 53), s9[43], rnd, sh), hbtf(t, c!(t, cospi, 43), s9[53], cn!(t, cospi, 21), s9[42], rnd, sh), hbtf(t, c!(t, cospi, 27), s9[54], cn!(t, cospi, 37), s9[41], rnd, sh), hbtf(t, c!(t, cospi, 59), s9[55], cn!(t, cospi, 5), s9[40], rnd, sh),
        hbtf(t, c!(t, cospi, 7), s9[56], cn!(t, cospi, 57), s9[39], rnd, sh), hbtf(t, c!(t, cospi, 39), s9[57], cn!(t, cospi, 25), s9[38], rnd, sh), hbtf(t, c!(t, cospi, 23), s9[58], cn!(t, cospi, 41), s9[37], rnd, sh), hbtf(t, c!(t, cospi, 55), s9[59], cn!(t, cospi, 9), s9[36], rnd, sh),
        hbtf(t, c!(t, cospi, 15), s9[60], cn!(t, cospi, 49), s9[35], rnd, sh), hbtf(t, c!(t, cospi, 47), s9[61], cn!(t, cospi, 17), s9[34], rnd, sh), hbtf(t, c!(t, cospi, 31), s9[62], cn!(t, cospi, 33), s9[33], rnd, sh), hbtf(t, c!(t, cospi, 63), s9[63], cn!(t, cospi, 1), s9[32], rnd, sh),
    ];
    // stage 11
    *out = [
        s10[0], s10[32], s10[16], s10[48],
        s10[8], s10[40], s10[24], s10[56],
        s10[4], s10[36], s10[20], s10[52],
        s10[12], s10[44], s10[28], s10[60],
        s10[2], s10[34], s10[18], s10[50],
        s10[10], s10[42], s10[26], s10[58],
        s10[6], s10[38], s10[22], s10[54],
        s10[14], s10[46], s10[30], s10[62],
        s10[1], s10[33], s10[17], s10[49],
        s10[9], s10[41], s10[25], s10[57],
        s10[5], s10[37], s10[21], s10[53],
        s10[13], s10[45], s10[29], s10[61],
        s10[3], s10[35], s10[19], s10[51],
        s10[11], s10[43], s10[27], s10[59],
        s10[7], s10[39], s10[23], s10[55],
        s10[15], s10[47], s10[31], s10[63],
    ];
}
#[rite]
pub(super) fn driver(
            t: Desktop64,
            input: &[i32],
            output: &mut [i32],
            input_stride: usize,
        ) {
            const N: usize = 64;
            const G: usize = N / 8;
            let shs = fwd_txfm_shift(N, N);
            let pre_col = -(shs[0] as i32); // round_shift_array arg (pre col)
            let post_col = -(shs[1] as i32); // post col
            let post_row = -(shs[2] as i32); // post row
            let txw = N.trailing_zeros() as usize - 2;
            let cos_bit_col = FWD_COS_BIT_COL[txw][txw];
            let cos_bit_row = FWD_COS_BIT_ROW[txw][txw];

            let mut buf = [0i32; N * N];

            // COLUMN PASS first — 8 columns at a time, contiguous.
            for cg in 0..G {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); N];
                for r in 0..N {
                    colin[r] =
                        round_shift_v(t, load8(t, input, r * input_stride + colbase), pre_col);
                }
                let mut colout = [_mm256_setzero_si256(); N];
                fdct64_x8(t, &colin, &mut colout, cos_bit_col);
                for r in 0..N {
                    let v = round_shift_v(t, colout[r], post_col);
                    store8(t, &mut buf, r * N + colbase, v);
                }
            }

            // ROW PASS — 8 rows at a time (transpose on load & store).
            for rg in 0..G {
                let rowbase = rg * 8;
                let mut pos = [_mm256_setzero_si256(); N];
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for l in 0..8 {
                        tile[l] = load8(t, &buf, (rowbase + l) * N + s * 8);
                    }
                    let tt = transpose8(t, &tile);
                    for j in 0..8 {
                        pos[s * 8 + j] = tt[j];
                    }
                }
                let mut rowout = [_mm256_setzero_si256(); N];
                fdct64_x8(t, &pos, &mut rowout, cos_bit_row);
                for i in 0..N {
                    rowout[i] = round_shift_v(t, rowout[i], post_row);
                }
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for j in 0..8 {
                        tile[j] = rowout[s * 8 + j];
                    }
                    let tt = transpose8(t, &tile);
                    for l in 0..8 {
                        store8(t, output, (rowbase + l) * N + s * 8, tt[l]);
                    }
                }
            }
        }
