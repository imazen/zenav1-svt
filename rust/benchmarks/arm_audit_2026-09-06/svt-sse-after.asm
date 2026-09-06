
rust/target/release/deps/kernel_tiers-9d59c30f0562a5e1:	file format mach-o arm64

Disassembly of section __TEXT,__text:

00000001000d5e0c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse>:
1000d5e0c: d10143ff    	sub	sp, sp, #0x50
1000d5e10: a9015ff8    	stp	x24, x23, [sp, #0x10]
1000d5e14: a90257f6    	stp	x22, x21, [sp, #0x20]
1000d5e18: a9034ff4    	stp	x20, x19, [sp, #0x30]
1000d5e1c: a9047bfd    	stp	x29, x30, [sp, #0x40]
1000d5e20: 910103fd    	add	x29, sp, #0x40
1000d5e24: 90000508    	adrp	x8, 0x100175000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
1000d5e28: 9101e108    	add	x8, x8, #0x78
1000d5e2c: 39400108    	ldrb	w8, [x8]
1000d5e30: 7100051f    	cmp	w8, #0x1
1000d5e34: 54002440    	b.eq	0x1000d62bc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x4b0>
1000d5e38: 7100091f    	cmp	w8, #0x2
1000d5e3c: 540021e1    	b.ne	0x1000d6278 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x46c>
1000d5e40: f10010df    	cmp	x6, #0x4
1000d5e44: 54000060    	b.eq	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x44>
1000d5e48: f10020df    	cmp	x6, #0x8
1000d5e4c: 540005a1    	b.ne	0x1000d5f00 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0xf4>
1000d5e50: b40034c7    	cbz	x7, 0x1000d64e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6dc>
1000d5e54: d2800009    	mov	x9, #0x0                ; =0
1000d5e58: d280000a    	mov	x10, #0x0               ; =0
1000d5e5c: d2800008    	mov	x8, #0x0                ; =0
1000d5e60: 1400000c    	b	0x1000d5e90 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x84>
1000d5e64: 9e670160    	fmov	d0, x11
1000d5e68: 9e670181    	fmov	d1, x12
1000d5e6c: 2e217400    	uabd.8b	v0, v0, v1
1000d5e70: 2e20c000    	umull.8h	v0, v0, v0
1000d5e74: 6e703800    	uaddlv.8h	s0, v0
1000d5e78: 1e26000b    	fmov	w11, s0
1000d5e7c: 8b0b0108    	add	x8, x8, x11
1000d5e80: 8b05014a    	add	x10, x10, x5
1000d5e84: 8b020129    	add	x9, x9, x2
1000d5e88: f10004e7    	subs	x7, x7, #0x1
1000d5e8c: 54003300    	b.eq	0x1000d64ec <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6e0>
1000d5e90: f10020df    	cmp	x6, #0x8
1000d5e94: 540001c1    	b.ne	0x1000d5ecc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0xc0>
1000d5e98: 9100212b    	add	x11, x9, #0x8
1000d5e9c: b100253f    	cmn	x9, #0x9
1000d5ea0: 54001c88    	b.hi	0x1000d6230 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x424>
1000d5ea4: eb01017f    	cmp	x11, x1
1000d5ea8: 54001c48    	b.hi	0x1000d6230 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x424>
1000d5eac: 9100214b    	add	x11, x10, #0x8
1000d5eb0: b100255f    	cmn	x10, #0x9
1000d5eb4: 54001d08    	b.hi	0x1000d6254 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x448>
1000d5eb8: eb04017f    	cmp	x11, x4
1000d5ebc: 54001cc8    	b.hi	0x1000d6254 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x448>
1000d5ec0: f869680b    	ldr	x11, [x0, x9]
1000d5ec4: f86a686c    	ldr	x12, [x3, x10]
1000d5ec8: 17ffffe7    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x58>
1000d5ecc: 9100112b    	add	x11, x9, #0x4
1000d5ed0: b100153f    	cmn	x9, #0x5
1000d5ed4: 54001b48    	b.hi	0x1000d623c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x430>
1000d5ed8: eb01017f    	cmp	x11, x1
1000d5edc: 54001b08    	b.hi	0x1000d623c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x430>
1000d5ee0: 9100114b    	add	x11, x10, #0x4
1000d5ee4: b100155f    	cmn	x10, #0x5
1000d5ee8: 54001bc8    	b.hi	0x1000d6260 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x454>
1000d5eec: eb04017f    	cmp	x11, x4
1000d5ef0: 54001b88    	b.hi	0x1000d6260 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x454>
1000d5ef4: b869680b    	ldr	w11, [x0, x9]
1000d5ef8: b86a686c    	ldr	w12, [x3, x10]
1000d5efc: 17ffffda    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x58>
1000d5f00: b4002f47    	cbz	x7, 0x1000d64e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6dc>
1000d5f04: d280000a    	mov	x10, #0x0               ; =0
1000d5f08: d280000b    	mov	x11, #0x0               ; =0
1000d5f0c: d280000c    	mov	x12, #0x0               ; =0
1000d5f10: d280000d    	mov	x13, #0x0               ; =0
1000d5f14: d2800015    	mov	x21, #0x0               ; =0
1000d5f18: d280000e    	mov	x14, #0x0               ; =0
1000d5f1c: d280000f    	mov	x15, #0x0               ; =0
1000d5f20: 91004010    	add	x16, x0, #0x10
1000d5f24: 91004071    	add	x17, x3, #0x10
1000d5f28: 52800413    	mov	w19, #0x20              ; =32
1000d5f2c: 1400000b    	b	0x1000d5f58 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x14c>
1000d5f30: 910005ef    	add	x15, x15, #0x1
1000d5f34: 8b0e012e    	add	x14, x9, x14
1000d5f38: 8b0501ad    	add	x13, x13, x5
1000d5f3c: 8b02018c    	add	x12, x12, x2
1000d5f40: cb02016b    	sub	x11, x11, x2
1000d5f44: cb05014a    	sub	x10, x10, x5
1000d5f48: 8b020210    	add	x16, x16, x2
1000d5f4c: 8b050231    	add	x17, x17, x5
1000d5f50: eb0701ff    	cmp	x15, x7
1000d5f54: 54001520    	b.eq	0x1000d61f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3ec>
1000d5f58: f10040df    	cmp	x6, #0x10
1000d5f5c: 540000c2    	b.hs	0x1000d5f74 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x168>
1000d5f60: d2800009    	mov	x9, #0x0                ; =0
1000d5f64: d2800016    	mov	x22, #0x0               ; =0
1000d5f68: eb0602df    	cmp	x22, x6
1000d5f6c: 54fffe22    	b.hs	0x1000d5f30 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x124>
1000d5f70: 14000020    	b	0x1000d5ff0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x1e4>
1000d5f74: d2800017    	mov	x23, #0x0               ; =0
1000d5f78: 8b0d0074    	add	x20, x3, x13
1000d5f7c: 8b0c0018    	add	x24, x0, x12
1000d5f80: 6f00e400    	movi.2d	v0, #0000000000000000
1000d5f84: 8b170188    	add	x8, x12, x23
1000d5f88: 91004109    	add	x9, x8, #0x10
1000d5f8c: b100451f    	cmn	x8, #0x11
1000d5f90: fa419122    	ccmp	x9, x1, #0x2, ls
1000d5f94: 54001368    	b.hi	0x1000d6200 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3f4>
1000d5f98: 8b1701a8    	add	x8, x13, x23
1000d5f9c: 91004109    	add	x9, x8, #0x10
1000d5fa0: b100451f    	cmn	x8, #0x11
1000d5fa4: 540013a8    	b.hi	0x1000d6218 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x40c>
1000d5fa8: eb04013f    	cmp	x9, x4
1000d5fac: 54001368    	b.hi	0x1000d6218 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x40c>
1000d5fb0: 3cf76b01    	ldr	q1, [x24, x23]
1000d5fb4: 3cf76a82    	ldr	q2, [x20, x23]
1000d5fb8: 6e227421    	uabd.16b	v1, v1, v2
1000d5fbc: 2e21c022    	umull.8h	v2, v1, v1
1000d5fc0: 6e606840    	uadalp.4s	v0, v2
1000d5fc4: 6e21c021    	umull2.8h	v1, v1, v1
1000d5fc8: 6e606820    	uadalp.4s	v0, v1
1000d5fcc: 910042f6    	add	x22, x23, #0x10
1000d5fd0: 910082e8    	add	x8, x23, #0x20
1000d5fd4: aa1603f7    	mov	x23, x22
1000d5fd8: eb06011f    	cmp	x8, x6
1000d5fdc: 54fffd49    	b.ls	0x1000d5f84 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x178>
1000d5fe0: 4eb1b800    	addv.4s	s0, v0
1000d5fe4: 1e260009    	fmov	w9, s0
1000d5fe8: eb0602df    	cmp	x22, x6
1000d5fec: 54fffa22    	b.hs	0x1000d5f30 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x124>
1000d5ff0: 9b0f7ca8    	mul	x8, x5, x15
1000d5ff4: 9b0f7c54    	mul	x20, x2, x15
1000d5ff8: 8b0802d7    	add	x23, x22, x8
1000d5ffc: eb17009f    	cmp	x4, x23
1000d6000: 9a978097    	csel	x23, x4, x23, hi
1000d6004: 8b160108    	add	x8, x8, x22
1000d6008: cb0802e8    	sub	x8, x23, x8
1000d600c: 8b1402d7    	add	x23, x22, x20
1000d6010: eb17003f    	cmp	x1, x23
1000d6014: 9a978037    	csel	x23, x1, x23, hi
1000d6018: 8b160294    	add	x20, x20, x22
1000d601c: cb1402f4    	sub	x20, x23, x20
1000d6020: eb14011f    	cmp	x8, x20
1000d6024: 9a943114    	csel	x20, x8, x20, lo
1000d6028: aa3603e8    	mvn	x8, x22
1000d602c: 8b0800c8    	add	x8, x6, x8
1000d6030: eb08029f    	cmp	x20, x8
1000d6034: 9a883294    	csel	x20, x20, x8, lo
1000d6038: 91000694    	add	x20, x20, #0x1
1000d603c: f100869f    	cmp	x20, #0x21
1000d6040: 54000062    	b.hs	0x1000d604c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x240>
1000d6044: aa1603f4    	mov	x20, x22
1000d6048: 1400005b    	b	0x1000d61b4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3a8>
1000d604c: f2401297    	ands	x23, x20, #0x1f
1000d6050: 9a970277    	csel	x23, x19, x23, eq
1000d6054: cb170294    	sub	x20, x20, x23
1000d6058: 8b1402d4    	add	x20, x22, x20
1000d605c: 6f00e401    	movi.2d	v1, #0000000000000000
1000d6060: 6f00e400    	movi.2d	v0, #0000000000000000
1000d6064: 4e081ea0    	mov.d	v0[0], x21
1000d6068: 8b0c02d5    	add	x21, x22, x12
1000d606c: eb15003f    	cmp	x1, x21
1000d6070: 9a958035    	csel	x21, x1, x21, hi
1000d6074: 8b0b02b5    	add	x21, x21, x11
1000d6078: cb1602b5    	sub	x21, x21, x22
1000d607c: 8b0d02d8    	add	x24, x22, x13
1000d6080: eb18009f    	cmp	x4, x24
1000d6084: 9a988098    	csel	x24, x4, x24, hi
1000d6088: 6f00e402    	movi.2d	v2, #0000000000000000
1000d608c: 6f00e404    	movi.2d	v4, #0000000000000000
1000d6090: 8b0a0318    	add	x24, x24, x10
1000d6094: cb160318    	sub	x24, x24, x22
1000d6098: 6f00e403    	movi.2d	v3, #0000000000000000
1000d609c: 6f00e406    	movi.2d	v6, #0000000000000000
1000d60a0: 6f00e405    	movi.2d	v5, #0000000000000000
1000d60a4: eb1802bf    	cmp	x21, x24
1000d60a8: 9a9832b5    	csel	x21, x21, x24, lo
1000d60ac: 6f00e410    	movi.2d	v16, #0000000000000000
1000d60b0: 6f00e407    	movi.2d	v7, #0000000000000000
1000d60b4: 6f00e411    	movi.2d	v17, #0000000000000000
1000d60b8: eb0802bf    	cmp	x21, x8
1000d60bc: 9a8832a8    	csel	x8, x21, x8, lo
1000d60c0: aa2803e8    	mvn	x8, x8
1000d60c4: 8b170108    	add	x8, x8, x23
1000d60c8: 8b160215    	add	x21, x16, x22
1000d60cc: 8b160236    	add	x22, x17, x22
1000d60d0: 6f00e413    	movi.2d	v19, #0000000000000000
1000d60d4: 6f00e416    	movi.2d	v22, #0000000000000000
1000d60d8: 6f00e412    	movi.2d	v18, #0000000000000000
1000d60dc: 6f00e415    	movi.2d	v21, #0000000000000000
1000d60e0: 6f00e414    	movi.2d	v20, #0000000000000000
1000d60e4: 6f00e417    	movi.2d	v23, #0000000000000000
1000d60e8: ad7fe6b8    	ldp	q24, q25, [x21, #-0x10]
1000d60ec: ad7feeda    	ldp	q26, q27, [x22, #-0x10]
1000d60f0: 2e3a231c    	usubl.8h	v28, v24, v26
1000d60f4: 6e3a2318    	usubl2.8h	v24, v24, v26
1000d60f8: 2e3b233a    	usubl.8h	v26, v25, v27
1000d60fc: 6e3b2339    	usubl2.8h	v25, v25, v27
1000d6100: 0e78c31b    	smull.4s	v27, v24, v24
1000d6104: 4e7cc39d    	smull2.4s	v29, v28, v28
1000d6108: 4e78c318    	smull2.4s	v24, v24, v24
1000d610c: 0e7cc39c    	smull.4s	v28, v28, v28
1000d6110: 0e79c33e    	smull.4s	v30, v25, v25
1000d6114: 4e7ac35f    	smull2.4s	v31, v26, v26
1000d6118: 4e79c339    	smull2.4s	v25, v25, v25
1000d611c: 0e7ac35a    	smull.4s	v26, v26, v26
1000d6120: 6ebd1084    	uaddw2.2d	v4, v4, v29
1000d6124: 6ebb10c6    	uaddw2.2d	v6, v6, v27
1000d6128: 2ebb1063    	uaddw.2d	v3, v3, v27
1000d612c: 2ebd1021    	uaddw.2d	v1, v1, v29
1000d6130: 6ebc1042    	uaddw2.2d	v2, v2, v28
1000d6134: 2eb810a5    	uaddw.2d	v5, v5, v24
1000d6138: 2ebc1000    	uaddw.2d	v0, v0, v28
1000d613c: 6eb81210    	uaddw2.2d	v16, v16, v24
1000d6140: 6ebf12d6    	uaddw2.2d	v22, v22, v31
1000d6144: 6ebe12b5    	uaddw2.2d	v21, v21, v30
1000d6148: 2ebe1252    	uaddw.2d	v18, v18, v30
1000d614c: 2ebf1273    	uaddw.2d	v19, v19, v31
1000d6150: 6eba1231    	uaddw2.2d	v17, v17, v26
1000d6154: 2eb91294    	uaddw.2d	v20, v20, v25
1000d6158: 2eba10e7    	uaddw.2d	v7, v7, v26
1000d615c: 6eb912f7    	uaddw2.2d	v23, v23, v25
1000d6160: 910082b5    	add	x21, x21, #0x20
1000d6164: 910082d6    	add	x22, x22, #0x20
1000d6168: b1008108    	adds	x8, x8, #0x20
1000d616c: 54fffbe1    	b.ne	0x1000d60e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x2dc>
1000d6170: 4ee486c4    	add.2d	v4, v22, v4
1000d6174: 4ef086f0    	add.2d	v16, v23, v16
1000d6178: 4ee28622    	add.2d	v2, v17, v2
1000d617c: 4ee686a6    	add.2d	v6, v21, v6
1000d6180: 4ee18661    	add.2d	v1, v19, v1
1000d6184: 4ee58685    	add.2d	v5, v20, v5
1000d6188: 4ee084e0    	add.2d	v0, v7, v0
1000d618c: 4ee38643    	add.2d	v3, v18, v3
1000d6190: 4ee38400    	add.2d	v0, v0, v3
1000d6194: 4ee58421    	add.2d	v1, v1, v5
1000d6198: 4ee18400    	add.2d	v0, v0, v1
1000d619c: 4ee68441    	add.2d	v1, v2, v6
1000d61a0: 4ef08482    	add.2d	v2, v4, v16
1000d61a4: 4ee28421    	add.2d	v1, v1, v2
1000d61a8: 4ee18400    	add.2d	v0, v0, v1
1000d61ac: 5ef1b800    	addp.2d	d0, v0
1000d61b0: 9e660015    	fmov	x21, d0
1000d61b4: 8b0d0076    	add	x22, x3, x13
1000d61b8: 8b0c0017    	add	x23, x0, x12
1000d61bc: 8b140188    	add	x8, x12, x20
1000d61c0: eb01011f    	cmp	x8, x1
1000d61c4: 54001be2    	b.hs	0x1000d6540 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x734>
1000d61c8: 8b1401a8    	add	x8, x13, x20
1000d61cc: eb04011f    	cmp	x8, x4
1000d61d0: 54001ae2    	b.hs	0x1000d652c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x720>
1000d61d4: 38746ae8    	ldrb	w8, [x23, x20]
1000d61d8: 38746ad8    	ldrb	w24, [x22, x20]
1000d61dc: 4b180108    	sub	w8, w8, w24
1000d61e0: 1b087d08    	mul	w8, w8, w8
1000d61e4: 8b0802b5    	add	x21, x21, x8
1000d61e8: 91000694    	add	x20, x20, #0x1
1000d61ec: eb1400df    	cmp	x6, x20
1000d61f0: 54fffe61    	b.ne	0x1000d61bc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3b0>
1000d61f4: 17ffff4f    	b	0x1000d5f30 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x124>
1000d61f8: 8b0e02a8    	add	x8, x21, x14
1000d61fc: 140000bc    	b	0x1000d64ec <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6e0>
1000d6200: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6204: 9133e063    	add	x3, x3, #0xcf8
1000d6208: aa0803e0    	mov	x0, x8
1000d620c: aa0103e2    	mov	x2, x1
1000d6210: aa0903e1    	mov	x1, x9
1000d6214: 940147ef    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d6218: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d621c: 91344063    	add	x3, x3, #0xd10
1000d6220: aa0803e0    	mov	x0, x8
1000d6224: aa0903e1    	mov	x1, x9
1000d6228: aa0403e2    	mov	x2, x4
1000d622c: 940147e9    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d6230: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6234: 9131a063    	add	x3, x3, #0xc68
1000d6238: 14000003    	b	0x1000d6244 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x438>
1000d623c: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6240: 91326063    	add	x3, x3, #0xc98
1000d6244: aa0903e0    	mov	x0, x9
1000d6248: aa0103e2    	mov	x2, x1
1000d624c: aa0b03e1    	mov	x1, x11
1000d6250: 940147e0    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d6254: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6258: 91320063    	add	x3, x3, #0xc80
1000d625c: 14000003    	b	0x1000d6268 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x45c>
1000d6260: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6264: 9132c063    	add	x3, x3, #0xcb0
1000d6268: aa0a03e0    	mov	x0, x10
1000d626c: aa0b03e1    	mov	x1, x11
1000d6270: aa0403e2    	mov	x2, x4
1000d6274: 940147d7    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d6278: a90003e4    	stp	x4, x0, [sp]
1000d627c: aa0103f8    	mov	x24, x1
1000d6280: aa0703f5    	mov	x21, x7
1000d6284: aa0603f3    	mov	x19, x6
1000d6288: aa0503f6    	mov	x22, x5
1000d628c: aa0303f4    	mov	x20, x3
1000d6290: aa0203f7    	mov	x23, x2
1000d6294: 940135cb    	bl	0x1001239c0 <__RNvNtNtNtCsfrLY33Z0RM3_8archmage6tokens9generated3arm11neon_detect>
1000d6298: aa1703e2    	mov	x2, x23
1000d629c: aa1403e3    	mov	x3, x20
1000d62a0: aa1603e5    	mov	x5, x22
1000d62a4: aa1303e6    	mov	x6, x19
1000d62a8: aa1503e7    	mov	x7, x21
1000d62ac: aa1803e1    	mov	x1, x24
1000d62b0: aa0003e8    	mov	x8, x0
1000d62b4: a94003e4    	ldp	x4, x0, [sp]
1000d62b8: 35ffdc48    	cbnz	w8, 0x1000d5e40 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x34>
1000d62bc: b4001167    	cbz	x7, 0x1000d64e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6dc>
1000d62c0: d280000a    	mov	x10, #0x0               ; =0
1000d62c4: d280000b    	mov	x11, #0x0               ; =0
1000d62c8: d280000c    	mov	x12, #0x0               ; =0
1000d62cc: d280000d    	mov	x13, #0x0               ; =0
1000d62d0: d2800008    	mov	x8, #0x0                ; =0
1000d62d4: d280000e    	mov	x14, #0x0               ; =0
1000d62d8: d10004cf    	sub	x15, x6, #0x1
1000d62dc: 91004070    	add	x16, x3, #0x10
1000d62e0: 91004011    	add	x17, x0, #0x10
1000d62e4: 52800413    	mov	w19, #0x20              ; =32
1000d62e8: 1400000a    	b	0x1000d6310 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x504>
1000d62ec: 910005ce    	add	x14, x14, #0x1
1000d62f0: 8b0201ad    	add	x13, x13, x2
1000d62f4: cb02018c    	sub	x12, x12, x2
1000d62f8: 8b05016b    	add	x11, x11, x5
1000d62fc: cb05014a    	sub	x10, x10, x5
1000d6300: 8b050210    	add	x16, x16, x5
1000d6304: 8b020231    	add	x17, x17, x2
1000d6308: eb0701df    	cmp	x14, x7
1000d630c: 54000f00    	b.eq	0x1000d64ec <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6e0>
1000d6310: eb0d003f    	cmp	x1, x13
1000d6314: 9a8d8029    	csel	x9, x1, x13, hi
1000d6318: 8b0c0129    	add	x9, x9, x12
1000d631c: eb0b009f    	cmp	x4, x11
1000d6320: 9a8b8094    	csel	x20, x4, x11, hi
1000d6324: 8b0a0294    	add	x20, x20, x10
1000d6328: eb14013f    	cmp	x9, x20
1000d632c: 9a943129    	csel	x9, x9, x20, lo
1000d6330: eb0f013f    	cmp	x9, x15
1000d6334: 9a8f3129    	csel	x9, x9, x15, lo
1000d6338: 9b0e7cb4    	mul	x20, x5, x14
1000d633c: eb140094    	subs	x20, x4, x20
1000d6340: 9a9433f4    	csel	x20, xzr, x20, lo
1000d6344: 9b0e7c55    	mul	x21, x2, x14
1000d6348: eb150035    	subs	x21, x1, x21
1000d634c: 9a9533f5    	csel	x21, xzr, x21, lo
1000d6350: eb15029f    	cmp	x20, x21
1000d6354: 9a953294    	csel	x20, x20, x21, lo
1000d6358: eb0f029f    	cmp	x20, x15
1000d635c: 9a8f3294    	csel	x20, x20, x15, lo
1000d6360: b4fffc66    	cbz	x6, 0x1000d62ec <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x4e0>
1000d6364: 91000694    	add	x20, x20, #0x1
1000d6368: f100869f    	cmp	x20, #0x21
1000d636c: 54000062    	b.hs	0x1000d6378 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x56c>
1000d6370: d2800014    	mov	x20, #0x0               ; =0
1000d6374: 1400004c    	b	0x1000d64a4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x698>
1000d6378: f2401295    	ands	x21, x20, #0x1f
1000d637c: 6f00e401    	movi.2d	v1, #0000000000000000
1000d6380: 6f00e400    	movi.2d	v0, #0000000000000000
1000d6384: 4e081d00    	mov.d	v0[0], x8
1000d6388: 9a950268    	csel	x8, x19, x21, eq
1000d638c: 6f00e402    	movi.2d	v2, #0000000000000000
1000d6390: cb080294    	sub	x20, x20, x8
1000d6394: 6f00e404    	movi.2d	v4, #0000000000000000
1000d6398: cb080128    	sub	x8, x9, x8
1000d639c: 6f00e403    	movi.2d	v3, #0000000000000000
1000d63a0: 91000508    	add	x8, x8, #0x1
1000d63a4: 6f00e405    	movi.2d	v5, #0000000000000000
1000d63a8: aa1103e9    	mov	x9, x17
1000d63ac: 6f00e406    	movi.2d	v6, #0000000000000000
1000d63b0: aa1003f5    	mov	x21, x16
1000d63b4: 6f00e411    	movi.2d	v17, #0000000000000000
1000d63b8: 6f00e407    	movi.2d	v7, #0000000000000000
1000d63bc: 6f00e413    	movi.2d	v19, #0000000000000000
1000d63c0: 6f00e412    	movi.2d	v18, #0000000000000000
1000d63c4: 6f00e416    	movi.2d	v22, #0000000000000000
1000d63c8: 6f00e410    	movi.2d	v16, #0000000000000000
1000d63cc: 6f00e415    	movi.2d	v21, #0000000000000000
1000d63d0: 6f00e414    	movi.2d	v20, #0000000000000000
1000d63d4: 6f00e417    	movi.2d	v23, #0000000000000000
1000d63d8: ad7fe538    	ldp	q24, q25, [x9, #-0x10]
1000d63dc: ad7feeba    	ldp	q26, q27, [x21, #-0x10]
1000d63e0: 2e3a231c    	usubl.8h	v28, v24, v26
1000d63e4: 6e3a2318    	usubl2.8h	v24, v24, v26
1000d63e8: 2e3b233a    	usubl.8h	v26, v25, v27
1000d63ec: 6e3b2339    	usubl2.8h	v25, v25, v27
1000d63f0: 0e78c31b    	smull.4s	v27, v24, v24
1000d63f4: 4e7cc39d    	smull2.4s	v29, v28, v28
1000d63f8: 4e78c318    	smull2.4s	v24, v24, v24
1000d63fc: 0e7cc39c    	smull.4s	v28, v28, v28
1000d6400: 0e79c33e    	smull.4s	v30, v25, v25
1000d6404: 4e7ac35f    	smull2.4s	v31, v26, v26
1000d6408: 4e79c339    	smull2.4s	v25, v25, v25
1000d640c: 0e7ac35a    	smull.4s	v26, v26, v26
1000d6410: 6ebd1084    	uaddw2.2d	v4, v4, v29
1000d6414: 6ebb10a5    	uaddw2.2d	v5, v5, v27
1000d6418: 2ebb1063    	uaddw.2d	v3, v3, v27
1000d641c: 2ebd1021    	uaddw.2d	v1, v1, v29
1000d6420: 6ebc1042    	uaddw2.2d	v2, v2, v28
1000d6424: 2eb810c6    	uaddw.2d	v6, v6, v24
1000d6428: 2ebc1000    	uaddw.2d	v0, v0, v28
1000d642c: 6eb81231    	uaddw2.2d	v17, v17, v24
1000d6430: 6ebf12d6    	uaddw2.2d	v22, v22, v31
1000d6434: 6ebe12b5    	uaddw2.2d	v21, v21, v30
1000d6438: 2ebe1210    	uaddw.2d	v16, v16, v30
1000d643c: 2ebf1252    	uaddw.2d	v18, v18, v31
1000d6440: 6eba1273    	uaddw2.2d	v19, v19, v26
1000d6444: 2eb91294    	uaddw.2d	v20, v20, v25
1000d6448: 2eba10e7    	uaddw.2d	v7, v7, v26
1000d644c: 6eb912f7    	uaddw2.2d	v23, v23, v25
1000d6450: 910082b5    	add	x21, x21, #0x20
1000d6454: 91008129    	add	x9, x9, #0x20
1000d6458: f1008108    	subs	x8, x8, #0x20
1000d645c: 54fffbe1    	b.ne	0x1000d63d8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5cc>
1000d6460: 4ee486c4    	add.2d	v4, v22, v4
1000d6464: 4ef186f1    	add.2d	v17, v23, v17
1000d6468: 4ee28662    	add.2d	v2, v19, v2
1000d646c: 4ee586a5    	add.2d	v5, v21, v5
1000d6470: 4ee18641    	add.2d	v1, v18, v1
1000d6474: 4ee68686    	add.2d	v6, v20, v6
1000d6478: 4ee084e0    	add.2d	v0, v7, v0
1000d647c: 4ee38603    	add.2d	v3, v16, v3
1000d6480: 4ee38400    	add.2d	v0, v0, v3
1000d6484: 4ee68421    	add.2d	v1, v1, v6
1000d6488: 4ee18400    	add.2d	v0, v0, v1
1000d648c: 4ee58441    	add.2d	v1, v2, v5
1000d6490: 4ef18482    	add.2d	v2, v4, v17
1000d6494: 4ee28421    	add.2d	v1, v1, v2
1000d6498: 4ee18400    	add.2d	v0, v0, v1
1000d649c: 5ef1b800    	addp.2d	d0, v0
1000d64a0: 9e660008    	fmov	x8, d0
1000d64a4: 8b0b0075    	add	x21, x3, x11
1000d64a8: 8b0d0016    	add	x22, x0, x13
1000d64ac: 8b1401a9    	add	x9, x13, x20
1000d64b0: eb01013f    	cmp	x9, x1
1000d64b4: 54000342    	b.hs	0x1000d651c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x710>
1000d64b8: 8b140169    	add	x9, x11, x20
1000d64bc: eb04013f    	cmp	x9, x4
1000d64c0: 54000242    	b.hs	0x1000d6508 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6fc>
1000d64c4: 38746ac9    	ldrb	w9, [x22, x20]
1000d64c8: 38746ab7    	ldrb	w23, [x21, x20]
1000d64cc: 91000694    	add	x20, x20, #0x1
1000d64d0: 4b170129    	sub	w9, w9, w23
1000d64d4: 1b097d29    	mul	w9, w9, w9
1000d64d8: 8b090108    	add	x8, x8, x9
1000d64dc: eb1400df    	cmp	x6, x20
1000d64e0: 54fffe61    	b.ne	0x1000d64ac <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x6a0>
1000d64e4: 17ffff82    	b	0x1000d62ec <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x4e0>
1000d64e8: d2800008    	mov	x8, #0x0                ; =0
1000d64ec: aa0803e0    	mov	x0, x8
1000d64f0: a9447bfd    	ldp	x29, x30, [sp, #0x40]
1000d64f4: a9434ff4    	ldp	x20, x19, [sp, #0x30]
1000d64f8: a94257f6    	ldp	x22, x21, [sp, #0x20]
1000d64fc: a9415ff8    	ldp	x24, x23, [sp, #0x10]
1000d6500: 910143ff    	add	sp, sp, #0x50
1000d6504: d65f03c0    	ret
1000d6508: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d650c: 91308042    	add	x2, x2, #0xc20
1000d6510: aa0903e0    	mov	x0, x9
1000d6514: aa0403e1    	mov	x1, x4
1000d6518: 940146c8    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d651c: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6520: 91302042    	add	x2, x2, #0xc08
1000d6524: aa0903e0    	mov	x0, x9
1000d6528: 940146c4    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d652c: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6530: 91338042    	add	x2, x2, #0xce0
1000d6534: aa0803e0    	mov	x0, x8
1000d6538: aa0403e1    	mov	x1, x4
1000d653c: 940146bf    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d6540: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6544: 91332042    	add	x2, x2, #0xcc8
1000d6548: aa0803e0    	mov	x0, x8
1000d654c: 940146bb    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
