
rust/target/release/deps/kernel_tiers-9d59c30f0562a5e1:	file format mach-o arm64

Disassembly of section __TEXT,__text:

00000001000d6694 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse>:
1000d6694: d10143ff    	sub	sp, sp, #0x50
1000d6698: a9015ff8    	stp	x24, x23, [sp, #0x10]
1000d669c: a90257f6    	stp	x22, x21, [sp, #0x20]
1000d66a0: a9034ff4    	stp	x20, x19, [sp, #0x30]
1000d66a4: a9047bfd    	stp	x29, x30, [sp, #0x40]
1000d66a8: 910103fd    	add	x29, sp, #0x40
1000d66ac: f00004e8    	adrp	x8, 0x100175000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
1000d66b0: 9101e108    	add	x8, x8, #0x78
1000d66b4: 39400108    	ldrb	w8, [x8]
1000d66b8: 7100051f    	cmp	w8, #0x1
1000d66bc: 54001c00    	b.eq	0x1000d6a3c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3a8>
1000d66c0: 7100091f    	cmp	w8, #0x2
1000d66c4: 540019a1    	b.ne	0x1000d69f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x364>
1000d66c8: b4002d07    	cbz	x7, 0x1000d6c68 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5d4>
1000d66cc: d280000a    	mov	x10, #0x0               ; =0
1000d66d0: d280000b    	mov	x11, #0x0               ; =0
1000d66d4: d280000c    	mov	x12, #0x0               ; =0
1000d66d8: d280000e    	mov	x14, #0x0               ; =0
1000d66dc: d280000d    	mov	x13, #0x0               ; =0
1000d66e0: d2800015    	mov	x21, #0x0               ; =0
1000d66e4: d280000f    	mov	x15, #0x0               ; =0
1000d66e8: 91004010    	add	x16, x0, #0x10
1000d66ec: 91004071    	add	x17, x3, #0x10
1000d66f0: 52800413    	mov	w19, #0x20              ; =32
1000d66f4: 1400000b    	b	0x1000d6720 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x8c>
1000d66f8: 910005ef    	add	x15, x15, #0x1
1000d66fc: 8b0d012d    	add	x13, x9, x13
1000d6700: 8b0501ce    	add	x14, x14, x5
1000d6704: 8b02018c    	add	x12, x12, x2
1000d6708: cb02016b    	sub	x11, x11, x2
1000d670c: cb05014a    	sub	x10, x10, x5
1000d6710: 8b020210    	add	x16, x16, x2
1000d6714: 8b050231    	add	x17, x17, x5
1000d6718: eb0701ff    	cmp	x15, x7
1000d671c: 54001520    	b.eq	0x1000d69c0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x32c>
1000d6720: f10040df    	cmp	x6, #0x10
1000d6724: 540000c2    	b.hs	0x1000d673c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0xa8>
1000d6728: d2800009    	mov	x9, #0x0                ; =0
1000d672c: d2800016    	mov	x22, #0x0               ; =0
1000d6730: eb0602df    	cmp	x22, x6
1000d6734: 54fffe22    	b.hs	0x1000d66f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x64>
1000d6738: 14000020    	b	0x1000d67b8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x124>
1000d673c: d2800017    	mov	x23, #0x0               ; =0
1000d6740: 8b0e0074    	add	x20, x3, x14
1000d6744: 8b0c0018    	add	x24, x0, x12
1000d6748: 6f00e400    	movi.2d	v0, #0000000000000000
1000d674c: 8b170188    	add	x8, x12, x23
1000d6750: 91004109    	add	x9, x8, #0x10
1000d6754: b100451f    	cmn	x8, #0x11
1000d6758: fa419122    	ccmp	x9, x1, #0x2, ls
1000d675c: 54001368    	b.hi	0x1000d69c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x334>
1000d6760: 8b1701c8    	add	x8, x14, x23
1000d6764: 91004109    	add	x9, x8, #0x10
1000d6768: b100451f    	cmn	x8, #0x11
1000d676c: 540013a8    	b.hi	0x1000d69e0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x34c>
1000d6770: eb04013f    	cmp	x9, x4
1000d6774: 54001368    	b.hi	0x1000d69e0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x34c>
1000d6778: 3cf76b01    	ldr	q1, [x24, x23]
1000d677c: 3cf76a82    	ldr	q2, [x20, x23]
1000d6780: 6e227421    	uabd.16b	v1, v1, v2
1000d6784: 2e21c022    	umull.8h	v2, v1, v1
1000d6788: 6e606840    	uadalp.4s	v0, v2
1000d678c: 6e21c021    	umull2.8h	v1, v1, v1
1000d6790: 6e606820    	uadalp.4s	v0, v1
1000d6794: 910042f6    	add	x22, x23, #0x10
1000d6798: 910082e8    	add	x8, x23, #0x20
1000d679c: aa1603f7    	mov	x23, x22
1000d67a0: eb06011f    	cmp	x8, x6
1000d67a4: 54fffd49    	b.ls	0x1000d674c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0xb8>
1000d67a8: 4eb1b800    	addv.4s	s0, v0
1000d67ac: 1e260009    	fmov	w9, s0
1000d67b0: eb0602df    	cmp	x22, x6
1000d67b4: 54fffa22    	b.hs	0x1000d66f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x64>
1000d67b8: 9b0f7ca8    	mul	x8, x5, x15
1000d67bc: 9b0f7c54    	mul	x20, x2, x15
1000d67c0: 8b0802d7    	add	x23, x22, x8
1000d67c4: eb17009f    	cmp	x4, x23
1000d67c8: 9a978097    	csel	x23, x4, x23, hi
1000d67cc: 8b160108    	add	x8, x8, x22
1000d67d0: cb0802e8    	sub	x8, x23, x8
1000d67d4: 8b1402d7    	add	x23, x22, x20
1000d67d8: eb17003f    	cmp	x1, x23
1000d67dc: 9a978037    	csel	x23, x1, x23, hi
1000d67e0: 8b160294    	add	x20, x20, x22
1000d67e4: cb1402f4    	sub	x20, x23, x20
1000d67e8: eb14011f    	cmp	x8, x20
1000d67ec: 9a943114    	csel	x20, x8, x20, lo
1000d67f0: aa3603e8    	mvn	x8, x22
1000d67f4: 8b0800c8    	add	x8, x6, x8
1000d67f8: eb08029f    	cmp	x20, x8
1000d67fc: 9a883294    	csel	x20, x20, x8, lo
1000d6800: 91000694    	add	x20, x20, #0x1
1000d6804: f100869f    	cmp	x20, #0x21
1000d6808: 54000062    	b.hs	0x1000d6814 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x180>
1000d680c: aa1603f4    	mov	x20, x22
1000d6810: 1400005b    	b	0x1000d697c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x2e8>
1000d6814: f2401297    	ands	x23, x20, #0x1f
1000d6818: 9a970277    	csel	x23, x19, x23, eq
1000d681c: cb170294    	sub	x20, x20, x23
1000d6820: 8b1402d4    	add	x20, x22, x20
1000d6824: 6f00e401    	movi.2d	v1, #0000000000000000
1000d6828: 6f00e400    	movi.2d	v0, #0000000000000000
1000d682c: 4e081ea0    	mov.d	v0[0], x21
1000d6830: 8b0c02d5    	add	x21, x22, x12
1000d6834: eb15003f    	cmp	x1, x21
1000d6838: 9a958035    	csel	x21, x1, x21, hi
1000d683c: 8b0b02b5    	add	x21, x21, x11
1000d6840: cb1602b5    	sub	x21, x21, x22
1000d6844: 8b0e02d8    	add	x24, x22, x14
1000d6848: eb18009f    	cmp	x4, x24
1000d684c: 9a988098    	csel	x24, x4, x24, hi
1000d6850: 6f00e402    	movi.2d	v2, #0000000000000000
1000d6854: 6f00e404    	movi.2d	v4, #0000000000000000
1000d6858: 8b0a0318    	add	x24, x24, x10
1000d685c: cb160318    	sub	x24, x24, x22
1000d6860: 6f00e403    	movi.2d	v3, #0000000000000000
1000d6864: 6f00e406    	movi.2d	v6, #0000000000000000
1000d6868: 6f00e405    	movi.2d	v5, #0000000000000000
1000d686c: eb1802bf    	cmp	x21, x24
1000d6870: 9a9832b5    	csel	x21, x21, x24, lo
1000d6874: 6f00e410    	movi.2d	v16, #0000000000000000
1000d6878: 6f00e407    	movi.2d	v7, #0000000000000000
1000d687c: 6f00e411    	movi.2d	v17, #0000000000000000
1000d6880: eb0802bf    	cmp	x21, x8
1000d6884: 9a8832a8    	csel	x8, x21, x8, lo
1000d6888: aa2803e8    	mvn	x8, x8
1000d688c: 8b170108    	add	x8, x8, x23
1000d6890: 8b160215    	add	x21, x16, x22
1000d6894: 8b160236    	add	x22, x17, x22
1000d6898: 6f00e413    	movi.2d	v19, #0000000000000000
1000d689c: 6f00e416    	movi.2d	v22, #0000000000000000
1000d68a0: 6f00e412    	movi.2d	v18, #0000000000000000
1000d68a4: 6f00e415    	movi.2d	v21, #0000000000000000
1000d68a8: 6f00e414    	movi.2d	v20, #0000000000000000
1000d68ac: 6f00e417    	movi.2d	v23, #0000000000000000
1000d68b0: ad7fe6b8    	ldp	q24, q25, [x21, #-0x10]
1000d68b4: ad7feeda    	ldp	q26, q27, [x22, #-0x10]
1000d68b8: 2e3a231c    	usubl.8h	v28, v24, v26
1000d68bc: 6e3a2318    	usubl2.8h	v24, v24, v26
1000d68c0: 2e3b233a    	usubl.8h	v26, v25, v27
1000d68c4: 6e3b2339    	usubl2.8h	v25, v25, v27
1000d68c8: 0e78c31b    	smull.4s	v27, v24, v24
1000d68cc: 4e7cc39d    	smull2.4s	v29, v28, v28
1000d68d0: 4e78c318    	smull2.4s	v24, v24, v24
1000d68d4: 0e7cc39c    	smull.4s	v28, v28, v28
1000d68d8: 0e79c33e    	smull.4s	v30, v25, v25
1000d68dc: 4e7ac35f    	smull2.4s	v31, v26, v26
1000d68e0: 4e79c339    	smull2.4s	v25, v25, v25
1000d68e4: 0e7ac35a    	smull.4s	v26, v26, v26
1000d68e8: 6ebd1084    	uaddw2.2d	v4, v4, v29
1000d68ec: 6ebb10c6    	uaddw2.2d	v6, v6, v27
1000d68f0: 2ebb1063    	uaddw.2d	v3, v3, v27
1000d68f4: 2ebd1021    	uaddw.2d	v1, v1, v29
1000d68f8: 6ebc1042    	uaddw2.2d	v2, v2, v28
1000d68fc: 2eb810a5    	uaddw.2d	v5, v5, v24
1000d6900: 2ebc1000    	uaddw.2d	v0, v0, v28
1000d6904: 6eb81210    	uaddw2.2d	v16, v16, v24
1000d6908: 6ebf12d6    	uaddw2.2d	v22, v22, v31
1000d690c: 6ebe12b5    	uaddw2.2d	v21, v21, v30
1000d6910: 2ebe1252    	uaddw.2d	v18, v18, v30
1000d6914: 2ebf1273    	uaddw.2d	v19, v19, v31
1000d6918: 6eba1231    	uaddw2.2d	v17, v17, v26
1000d691c: 2eb91294    	uaddw.2d	v20, v20, v25
1000d6920: 2eba10e7    	uaddw.2d	v7, v7, v26
1000d6924: 6eb912f7    	uaddw2.2d	v23, v23, v25
1000d6928: 910082b5    	add	x21, x21, #0x20
1000d692c: 910082d6    	add	x22, x22, #0x20
1000d6930: b1008108    	adds	x8, x8, #0x20
1000d6934: 54fffbe1    	b.ne	0x1000d68b0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x21c>
1000d6938: 4ee486c4    	add.2d	v4, v22, v4
1000d693c: 4ef086f0    	add.2d	v16, v23, v16
1000d6940: 4ee28622    	add.2d	v2, v17, v2
1000d6944: 4ee686a6    	add.2d	v6, v21, v6
1000d6948: 4ee18661    	add.2d	v1, v19, v1
1000d694c: 4ee58685    	add.2d	v5, v20, v5
1000d6950: 4ee084e0    	add.2d	v0, v7, v0
1000d6954: 4ee38643    	add.2d	v3, v18, v3
1000d6958: 4ee38400    	add.2d	v0, v0, v3
1000d695c: 4ee58421    	add.2d	v1, v1, v5
1000d6960: 4ee18400    	add.2d	v0, v0, v1
1000d6964: 4ee68441    	add.2d	v1, v2, v6
1000d6968: 4ef08482    	add.2d	v2, v4, v16
1000d696c: 4ee28421    	add.2d	v1, v1, v2
1000d6970: 4ee18400    	add.2d	v0, v0, v1
1000d6974: 5ef1b800    	addp.2d	d0, v0
1000d6978: 9e660015    	fmov	x21, d0
1000d697c: 8b0e0076    	add	x22, x3, x14
1000d6980: 8b0c0017    	add	x23, x0, x12
1000d6984: 8b140188    	add	x8, x12, x20
1000d6988: eb01011f    	cmp	x8, x1
1000d698c: 540019a2    	b.hs	0x1000d6cc0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x62c>
1000d6990: 8b1401c8    	add	x8, x14, x20
1000d6994: eb04011f    	cmp	x8, x4
1000d6998: 540018a2    	b.hs	0x1000d6cac <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x618>
1000d699c: 38746ae8    	ldrb	w8, [x23, x20]
1000d69a0: 38746ad8    	ldrb	w24, [x22, x20]
1000d69a4: 4b180108    	sub	w8, w8, w24
1000d69a8: 1b087d08    	mul	w8, w8, w8
1000d69ac: 8b0802b5    	add	x21, x21, x8
1000d69b0: 91000694    	add	x20, x20, #0x1
1000d69b4: eb1400df    	cmp	x6, x20
1000d69b8: 54fffe61    	b.ne	0x1000d6984 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x2f0>
1000d69bc: 17ffff4f    	b	0x1000d66f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x64>
1000d69c0: 8b0d02a8    	add	x8, x21, x13
1000d69c4: 140000aa    	b	0x1000d6c6c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5d8>
1000d69c8: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d69cc: 91362063    	add	x3, x3, #0xd88
1000d69d0: aa0803e0    	mov	x0, x8
1000d69d4: aa0103e2    	mov	x2, x1
1000d69d8: aa0903e1    	mov	x1, x9
1000d69dc: 940145f0    	bl	0x10012819c <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d69e0: f00004a3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d69e4: 91368063    	add	x3, x3, #0xda0
1000d69e8: aa0803e0    	mov	x0, x8
1000d69ec: aa0903e1    	mov	x1, x9
1000d69f0: aa0403e2    	mov	x2, x4
1000d69f4: 940145ea    	bl	0x10012819c <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d69f8: a90003e4    	stp	x4, x0, [sp]
1000d69fc: aa0103f8    	mov	x24, x1
1000d6a00: aa0703f5    	mov	x21, x7
1000d6a04: aa0603f3    	mov	x19, x6
1000d6a08: aa0503f6    	mov	x22, x5
1000d6a0c: aa0303f4    	mov	x20, x3
1000d6a10: aa0203f7    	mov	x23, x2
1000d6a14: 940133de    	bl	0x10012398c <__RNvNtNtNtCsfrLY33Z0RM3_8archmage6tokens9generated3arm11neon_detect>
1000d6a18: aa1703e2    	mov	x2, x23
1000d6a1c: aa1403e3    	mov	x3, x20
1000d6a20: aa1603e5    	mov	x5, x22
1000d6a24: aa1303e6    	mov	x6, x19
1000d6a28: aa1503e7    	mov	x7, x21
1000d6a2c: aa1803e1    	mov	x1, x24
1000d6a30: aa0003e8    	mov	x8, x0
1000d6a34: a94003e4    	ldp	x4, x0, [sp]
1000d6a38: 35ffe488    	cbnz	w8, 0x1000d66c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x34>
1000d6a3c: b4001167    	cbz	x7, 0x1000d6c68 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5d4>
1000d6a40: d280000a    	mov	x10, #0x0               ; =0
1000d6a44: d280000b    	mov	x11, #0x0               ; =0
1000d6a48: d280000c    	mov	x12, #0x0               ; =0
1000d6a4c: d280000d    	mov	x13, #0x0               ; =0
1000d6a50: d2800008    	mov	x8, #0x0                ; =0
1000d6a54: d280000e    	mov	x14, #0x0               ; =0
1000d6a58: d10004cf    	sub	x15, x6, #0x1
1000d6a5c: 91004070    	add	x16, x3, #0x10
1000d6a60: 91004011    	add	x17, x0, #0x10
1000d6a64: 52800413    	mov	w19, #0x20              ; =32
1000d6a68: 1400000a    	b	0x1000d6a90 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3fc>
1000d6a6c: 910005ce    	add	x14, x14, #0x1
1000d6a70: 8b0201ad    	add	x13, x13, x2
1000d6a74: cb02018c    	sub	x12, x12, x2
1000d6a78: 8b05016b    	add	x11, x11, x5
1000d6a7c: cb05014a    	sub	x10, x10, x5
1000d6a80: 8b050210    	add	x16, x16, x5
1000d6a84: 8b020231    	add	x17, x17, x2
1000d6a88: eb0701df    	cmp	x14, x7
1000d6a8c: 54000f00    	b.eq	0x1000d6c6c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5d8>
1000d6a90: eb0d003f    	cmp	x1, x13
1000d6a94: 9a8d8029    	csel	x9, x1, x13, hi
1000d6a98: 8b0c0129    	add	x9, x9, x12
1000d6a9c: eb0b009f    	cmp	x4, x11
1000d6aa0: 9a8b8094    	csel	x20, x4, x11, hi
1000d6aa4: 8b0a0294    	add	x20, x20, x10
1000d6aa8: eb14013f    	cmp	x9, x20
1000d6aac: 9a943129    	csel	x9, x9, x20, lo
1000d6ab0: eb0f013f    	cmp	x9, x15
1000d6ab4: 9a8f3129    	csel	x9, x9, x15, lo
1000d6ab8: 9b0e7cb4    	mul	x20, x5, x14
1000d6abc: eb140094    	subs	x20, x4, x20
1000d6ac0: 9a9433f4    	csel	x20, xzr, x20, lo
1000d6ac4: 9b0e7c55    	mul	x21, x2, x14
1000d6ac8: eb150035    	subs	x21, x1, x21
1000d6acc: 9a9533f5    	csel	x21, xzr, x21, lo
1000d6ad0: eb15029f    	cmp	x20, x21
1000d6ad4: 9a953294    	csel	x20, x20, x21, lo
1000d6ad8: eb0f029f    	cmp	x20, x15
1000d6adc: 9a8f3294    	csel	x20, x20, x15, lo
1000d6ae0: b4fffc66    	cbz	x6, 0x1000d6a6c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3d8>
1000d6ae4: 91000694    	add	x20, x20, #0x1
1000d6ae8: f100869f    	cmp	x20, #0x21
1000d6aec: 54000062    	b.hs	0x1000d6af8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x464>
1000d6af0: d2800014    	mov	x20, #0x0               ; =0
1000d6af4: 1400004c    	b	0x1000d6c24 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x590>
1000d6af8: f2401295    	ands	x21, x20, #0x1f
1000d6afc: 6f00e401    	movi.2d	v1, #0000000000000000
1000d6b00: 6f00e400    	movi.2d	v0, #0000000000000000
1000d6b04: 4e081d00    	mov.d	v0[0], x8
1000d6b08: 9a950268    	csel	x8, x19, x21, eq
1000d6b0c: 6f00e402    	movi.2d	v2, #0000000000000000
1000d6b10: cb080294    	sub	x20, x20, x8
1000d6b14: 6f00e404    	movi.2d	v4, #0000000000000000
1000d6b18: cb080128    	sub	x8, x9, x8
1000d6b1c: 6f00e403    	movi.2d	v3, #0000000000000000
1000d6b20: 91000508    	add	x8, x8, #0x1
1000d6b24: 6f00e405    	movi.2d	v5, #0000000000000000
1000d6b28: aa1103e9    	mov	x9, x17
1000d6b2c: 6f00e406    	movi.2d	v6, #0000000000000000
1000d6b30: aa1003f5    	mov	x21, x16
1000d6b34: 6f00e411    	movi.2d	v17, #0000000000000000
1000d6b38: 6f00e407    	movi.2d	v7, #0000000000000000
1000d6b3c: 6f00e413    	movi.2d	v19, #0000000000000000
1000d6b40: 6f00e412    	movi.2d	v18, #0000000000000000
1000d6b44: 6f00e416    	movi.2d	v22, #0000000000000000
1000d6b48: 6f00e410    	movi.2d	v16, #0000000000000000
1000d6b4c: 6f00e415    	movi.2d	v21, #0000000000000000
1000d6b50: 6f00e414    	movi.2d	v20, #0000000000000000
1000d6b54: 6f00e417    	movi.2d	v23, #0000000000000000
1000d6b58: ad7fe538    	ldp	q24, q25, [x9, #-0x10]
1000d6b5c: ad7feeba    	ldp	q26, q27, [x21, #-0x10]
1000d6b60: 2e3a231c    	usubl.8h	v28, v24, v26
1000d6b64: 6e3a2318    	usubl2.8h	v24, v24, v26
1000d6b68: 2e3b233a    	usubl.8h	v26, v25, v27
1000d6b6c: 6e3b2339    	usubl2.8h	v25, v25, v27
1000d6b70: 0e78c31b    	smull.4s	v27, v24, v24
1000d6b74: 4e7cc39d    	smull2.4s	v29, v28, v28
1000d6b78: 4e78c318    	smull2.4s	v24, v24, v24
1000d6b7c: 0e7cc39c    	smull.4s	v28, v28, v28
1000d6b80: 0e79c33e    	smull.4s	v30, v25, v25
1000d6b84: 4e7ac35f    	smull2.4s	v31, v26, v26
1000d6b88: 4e79c339    	smull2.4s	v25, v25, v25
1000d6b8c: 0e7ac35a    	smull.4s	v26, v26, v26
1000d6b90: 6ebd1084    	uaddw2.2d	v4, v4, v29
1000d6b94: 6ebb10a5    	uaddw2.2d	v5, v5, v27
1000d6b98: 2ebb1063    	uaddw.2d	v3, v3, v27
1000d6b9c: 2ebd1021    	uaddw.2d	v1, v1, v29
1000d6ba0: 6ebc1042    	uaddw2.2d	v2, v2, v28
1000d6ba4: 2eb810c6    	uaddw.2d	v6, v6, v24
1000d6ba8: 2ebc1000    	uaddw.2d	v0, v0, v28
1000d6bac: 6eb81231    	uaddw2.2d	v17, v17, v24
1000d6bb0: 6ebf12d6    	uaddw2.2d	v22, v22, v31
1000d6bb4: 6ebe12b5    	uaddw2.2d	v21, v21, v30
1000d6bb8: 2ebe1210    	uaddw.2d	v16, v16, v30
1000d6bbc: 2ebf1252    	uaddw.2d	v18, v18, v31
1000d6bc0: 6eba1273    	uaddw2.2d	v19, v19, v26
1000d6bc4: 2eb91294    	uaddw.2d	v20, v20, v25
1000d6bc8: 2eba10e7    	uaddw.2d	v7, v7, v26
1000d6bcc: 6eb912f7    	uaddw2.2d	v23, v23, v25
1000d6bd0: 910082b5    	add	x21, x21, #0x20
1000d6bd4: 91008129    	add	x9, x9, #0x20
1000d6bd8: f1008108    	subs	x8, x8, #0x20
1000d6bdc: 54fffbe1    	b.ne	0x1000d6b58 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x4c4>
1000d6be0: 4ee486c4    	add.2d	v4, v22, v4
1000d6be4: 4ef186f1    	add.2d	v17, v23, v17
1000d6be8: 4ee28662    	add.2d	v2, v19, v2
1000d6bec: 4ee586a5    	add.2d	v5, v21, v5
1000d6bf0: 4ee18641    	add.2d	v1, v18, v1
1000d6bf4: 4ee68686    	add.2d	v6, v20, v6
1000d6bf8: 4ee084e0    	add.2d	v0, v7, v0
1000d6bfc: 4ee38603    	add.2d	v3, v16, v3
1000d6c00: 4ee38400    	add.2d	v0, v0, v3
1000d6c04: 4ee68421    	add.2d	v1, v1, v6
1000d6c08: 4ee18400    	add.2d	v0, v0, v1
1000d6c0c: 4ee58441    	add.2d	v1, v2, v5
1000d6c10: 4ef18482    	add.2d	v2, v4, v17
1000d6c14: 4ee28421    	add.2d	v1, v1, v2
1000d6c18: 4ee18400    	add.2d	v0, v0, v1
1000d6c1c: 5ef1b800    	addp.2d	d0, v0
1000d6c20: 9e660008    	fmov	x8, d0
1000d6c24: 8b0b0075    	add	x21, x3, x11
1000d6c28: 8b0d0016    	add	x22, x0, x13
1000d6c2c: 8b1401a9    	add	x9, x13, x20
1000d6c30: eb01013f    	cmp	x9, x1
1000d6c34: 54000342    	b.hs	0x1000d6c9c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x608>
1000d6c38: 8b140169    	add	x9, x11, x20
1000d6c3c: eb04013f    	cmp	x9, x4
1000d6c40: 54000242    	b.hs	0x1000d6c88 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x5f4>
1000d6c44: 38746ac9    	ldrb	w9, [x22, x20]
1000d6c48: 38746ab7    	ldrb	w23, [x21, x20]
1000d6c4c: 91000694    	add	x20, x20, #0x1
1000d6c50: 4b170129    	sub	w9, w9, w23
1000d6c54: 1b097d29    	mul	w9, w9, w9
1000d6c58: 8b090108    	add	x8, x8, x9
1000d6c5c: eb1400df    	cmp	x6, x20
1000d6c60: 54fffe61    	b.ne	0x1000d6c2c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x598>
1000d6c64: 17ffff82    	b	0x1000d6a6c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8variance3sse+0x3d8>
1000d6c68: d2800008    	mov	x8, #0x0                ; =0
1000d6c6c: aa0803e0    	mov	x0, x8
1000d6c70: a9447bfd    	ldp	x29, x30, [sp, #0x40]
1000d6c74: a9434ff4    	ldp	x20, x19, [sp, #0x30]
1000d6c78: a94257f6    	ldp	x22, x21, [sp, #0x20]
1000d6c7c: a9415ff8    	ldp	x24, x23, [sp, #0x10]
1000d6c80: 910143ff    	add	sp, sp, #0x50
1000d6c84: d65f03c0    	ret
1000d6c88: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6c8c: 91344042    	add	x2, x2, #0xd10
1000d6c90: aa0903e0    	mov	x0, x9
1000d6c94: aa0403e1    	mov	x1, x4
1000d6c98: 940144db    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d6c9c: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6ca0: 9133e042    	add	x2, x2, #0xcf8
1000d6ca4: aa0903e0    	mov	x0, x9
1000d6ca8: 940144d7    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d6cac: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6cb0: 9135c042    	add	x2, x2, #0xd70
1000d6cb4: aa0803e0    	mov	x0, x8
1000d6cb8: aa0403e1    	mov	x1, x4
1000d6cbc: 940144d2    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d6cc0: f00004a2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d6cc4: 91356042    	add	x2, x2, #0xd58
1000d6cc8: aa0803e0    	mov	x0, x8
1000d6ccc: 940144ce    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
