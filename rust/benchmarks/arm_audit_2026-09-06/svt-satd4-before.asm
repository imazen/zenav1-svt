
rust/target/release/deps/kernel_tiers-9d59c30f0562a5e1:	file format mach-o arm64

Disassembly of section __TEXT,__text:

00000001000d5650 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4>:
1000d5650: a9ba6ffc    	stp	x28, x27, [sp, #-0x60]!
1000d5654: a90167fa    	stp	x26, x25, [sp, #0x10]
1000d5658: a9025ff8    	stp	x24, x23, [sp, #0x20]
1000d565c: a90357f6    	stp	x22, x21, [sp, #0x30]
1000d5660: a9044ff4    	stp	x20, x19, [sp, #0x40]
1000d5664: a9057bfd    	stp	x29, x30, [sp, #0x50]
1000d5668: 910143fd    	add	x29, sp, #0x50
1000d566c: 90000508    	adrp	x8, 0x100175000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
1000d5670: 9101e108    	add	x8, x8, #0x78
1000d5674: 39400108    	ldrb	w8, [x8]
1000d5678: 7100051f    	cmp	w8, #0x1
1000d567c: 54001780    	b.eq	0x1000d596c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x31c>
1000d5680: 7100091f    	cmp	w8, #0x2
1000d5684: 54001561    	b.ne	0x1000d5930 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2e0>
1000d5688: f1000c3f    	cmp	x1, #0x3
1000d568c: 54001229    	b.ls	0x1000d58d0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x280>
1000d5690: f1000c9f    	cmp	x4, #0x3
1000d5694: 54001249    	b.ls	0x1000d58dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x28c>
1000d5698: 91001049    	add	x9, x2, #0x4
1000d569c: b100145f    	cmn	x2, #0x5
1000d56a0: 54001248    	b.hi	0x1000d58e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x298>
1000d56a4: eb01013f    	cmp	x9, x1
1000d56a8: 54001208    	b.hi	0x1000d58e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x298>
1000d56ac: 910010aa    	add	x10, x5, #0x4
1000d56b0: b10014bf    	cmn	x5, #0x5
1000d56b4: 540011e8    	b.hi	0x1000d58f0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2a0>
1000d56b8: eb04015f    	cmp	x10, x4
1000d56bc: 540011a8    	b.hi	0x1000d58f0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2a0>
1000d56c0: d37ff846    	lsl	x6, x2, #1
1000d56c4: 910010c9    	add	x9, x6, #0x4
1000d56c8: eb01013f    	cmp	x9, x1
1000d56cc: 54001168    	b.hi	0x1000d58f8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2a8>
1000d56d0: d37ff8a7    	lsl	x7, x5, #1
1000d56d4: 910010ea    	add	x10, x7, #0x4
1000d56d8: eb04015f    	cmp	x10, x4
1000d56dc: 540011c8    	b.hi	0x1000d5914 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2c4>
1000d56e0: 8b020448    	add	x8, x2, x2, lsl #1
1000d56e4: 91001109    	add	x9, x8, #0x4
1000d56e8: b100151f    	cmn	x8, #0x5
1000d56ec: 54001088    	b.hi	0x1000d58fc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2ac>
1000d56f0: eb01013f    	cmp	x9, x1
1000d56f4: 54001048    	b.hi	0x1000d58fc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2ac>
1000d56f8: 8b0504a9    	add	x9, x5, x5, lsl #1
1000d56fc: 9100112a    	add	x10, x9, #0x4
1000d5700: b100153f    	cmn	x9, #0x5
1000d5704: 540010a8    	b.hi	0x1000d5918 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2c8>
1000d5708: eb04015f    	cmp	x10, x4
1000d570c: 54001068    	b.hi	0x1000d5918 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2c8>
1000d5710: 3940000a    	ldrb	w10, [x0]
1000d5714: 3940006b    	ldrb	w11, [x3]
1000d5718: 3940040c    	ldrb	w12, [x0, #0x1]
1000d571c: 3940046d    	ldrb	w13, [x3, #0x1]
1000d5720: 3940080e    	ldrb	w14, [x0, #0x2]
1000d5724: 3940086f    	ldrb	w15, [x3, #0x2]
1000d5728: 39400c10    	ldrb	w16, [x0, #0x3]
1000d572c: 39400c71    	ldrb	w17, [x3, #0x3]
1000d5730: 8b020013    	add	x19, x0, x2
1000d5734: 8b050074    	add	x20, x3, x5
1000d5738: 39400261    	ldrb	w1, [x19]
1000d573c: 39400282    	ldrb	w2, [x20]
1000d5740: 39400664    	ldrb	w4, [x19, #0x1]
1000d5744: 39400685    	ldrb	w5, [x20, #0x1]
1000d5748: 39400a75    	ldrb	w21, [x19, #0x2]
1000d574c: 39400a96    	ldrb	w22, [x20, #0x2]
1000d5750: 39400e73    	ldrb	w19, [x19, #0x3]
1000d5754: 39400e94    	ldrb	w20, [x20, #0x3]
1000d5758: 8b060006    	add	x6, x0, x6
1000d575c: 8b070067    	add	x7, x3, x7
1000d5760: 394000d7    	ldrb	w23, [x6]
1000d5764: 394000f8    	ldrb	w24, [x7]
1000d5768: 394004d9    	ldrb	w25, [x6, #0x1]
1000d576c: 394004fa    	ldrb	w26, [x7, #0x1]
1000d5770: 394008db    	ldrb	w27, [x6, #0x2]
1000d5774: 394008fc    	ldrb	w28, [x7, #0x2]
1000d5778: 39400cc6    	ldrb	w6, [x6, #0x3]
1000d577c: 39400ce7    	ldrb	w7, [x7, #0x3]
1000d5780: 4b0700c6    	sub	w6, w6, w7
1000d5784: 4b1c0367    	sub	w7, w27, w28
1000d5788: d3603ce7    	ubfiz	x7, x7, #32, #16
1000d578c: aa06c0e6    	orr	x6, x7, x6, lsl #48
1000d5790: 4b1a0327    	sub	w7, w25, w26
1000d5794: 53103ce7    	lsl	w7, w7, #16
1000d5798: aa0700c6    	orr	x6, x6, x7
1000d579c: 4b1802e7    	sub	w7, w23, w24
1000d57a0: b3403ce6    	bfxil	x6, x7, #0, #16
1000d57a4: 9e6700c0    	fmov	d0, x6
1000d57a8: 4b140266    	sub	w6, w19, w20
1000d57ac: 4b1602a7    	sub	w7, w21, w22
1000d57b0: d3603ce7    	ubfiz	x7, x7, #32, #16
1000d57b4: aa06c0e6    	orr	x6, x7, x6, lsl #48
1000d57b8: 4b050084    	sub	w4, w4, w5
1000d57bc: 53103c84    	lsl	w4, w4, #16
1000d57c0: aa0400c4    	orr	x4, x6, x4
1000d57c4: 4b020021    	sub	w1, w1, w2
1000d57c8: b3403c24    	bfxil	x4, x1, #0, #16
1000d57cc: 9e670081    	fmov	d1, x4
1000d57d0: 4b110210    	sub	w16, w16, w17
1000d57d4: 4b0f01ce    	sub	w14, w14, w15
1000d57d8: d3603dce    	ubfiz	x14, x14, #32, #16
1000d57dc: aa10c1ce    	orr	x14, x14, x16, lsl #48
1000d57e0: 4b0d018c    	sub	w12, w12, w13
1000d57e4: 53103d8c    	lsl	w12, w12, #16
1000d57e8: aa0c01cc    	orr	x12, x14, x12
1000d57ec: 4b0b014a    	sub	w10, w10, w11
1000d57f0: b3403d4c    	bfxil	x12, x10, #0, #16
1000d57f4: 9e670182    	fmov	d2, x12
1000d57f8: 8b080008    	add	x8, x0, x8
1000d57fc: 8b090069    	add	x9, x3, x9
1000d5800: 3940010a    	ldrb	w10, [x8]
1000d5804: 3940012b    	ldrb	w11, [x9]
1000d5808: 4b0b014a    	sub	w10, w10, w11
1000d580c: 3940050b    	ldrb	w11, [x8, #0x1]
1000d5810: 3940052c    	ldrb	w12, [x9, #0x1]
1000d5814: 4b0c016b    	sub	w11, w11, w12
1000d5818: 3940090c    	ldrb	w12, [x8, #0x2]
1000d581c: 3940092d    	ldrb	w13, [x9, #0x2]
1000d5820: 4b0d018c    	sub	w12, w12, w13
1000d5824: 39400d08    	ldrb	w8, [x8, #0x3]
1000d5828: 39400d29    	ldrb	w9, [x9, #0x3]
1000d582c: 4b090108    	sub	w8, w8, w9
1000d5830: d3603d89    	ubfiz	x9, x12, #32, #16
1000d5834: aa08c128    	orr	x8, x9, x8, lsl #48
1000d5838: 53103d69    	lsl	w9, w11, #16
1000d583c: aa090108    	orr	x8, x8, x9
1000d5840: b3403d48    	bfxil	x8, x10, #0, #16
1000d5844: 9e670103    	fmov	d3, x8
1000d5848: 0e628424    	add.4h	v4, v1, v2
1000d584c: 2e618441    	sub.4h	v1, v2, v1
1000d5850: 0e608462    	add.4h	v2, v3, v0
1000d5854: 2e638400    	sub.4h	v0, v0, v3
1000d5858: 0e648443    	add.4h	v3, v2, v4
1000d585c: 0e618405    	add.4h	v5, v0, v1
1000d5860: 2e628482    	sub.4h	v2, v4, v2
1000d5864: 2e608420    	sub.4h	v0, v1, v0
1000d5868: 0e452861    	trn1.4h	v1, v3, v5
1000d586c: 0e456863    	trn2.4h	v3, v3, v5
1000d5870: 0e402844    	trn1.4h	v4, v2, v0
1000d5874: 0e406840    	trn2.4h	v0, v2, v0
1000d5878: 0e843822    	zip1.2s	v2, v1, v4
1000d587c: 0e803865    	zip1.2s	v5, v3, v0
1000d5880: 0e847821    	zip2.2s	v1, v1, v4
1000d5884: 0e807860    	zip2.2s	v0, v3, v0
1000d5888: 0e658443    	add.4h	v3, v2, v5
1000d588c: 2e658442    	sub.4h	v2, v2, v5
1000d5890: 0e608424    	add.4h	v4, v1, v0
1000d5894: 2e608420    	sub.4h	v0, v1, v0
1000d5898: 0e648461    	add.4h	v1, v3, v4
1000d589c: 0e608445    	add.4h	v5, v2, v0
1000d58a0: 2e648463    	sub.4h	v3, v3, v4
1000d58a4: 2e608440    	sub.4h	v0, v2, v0
1000d58a8: 0e60b821    	abs.4h	v1, v1
1000d58ac: 0e60b8a2    	abs.4h	v2, v5
1000d58b0: 2e620021    	uaddl.4s	v1, v1, v2
1000d58b4: 6f00e402    	movi.2d	v2, #0000000000000000
1000d58b8: 0e625061    	sabal.4s	v1, v3, v2
1000d58bc: 0e625001    	sabal.4s	v1, v0, v2
1000d58c0: 4eb1b820    	addv.4s	s0, v1
1000d58c4: 1e260008    	fmov	w8, s0
1000d58c8: 11000508    	add	w8, w8, #0x1
1000d58cc: 1400010d    	b	0x1000d5d00 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6b0>
1000d58d0: d2800008    	mov	x8, #0x0                ; =0
1000d58d4: 52800089    	mov	w9, #0x4                ; =4
1000d58d8: 14000009    	b	0x1000d58fc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2ac>
1000d58dc: d2800009    	mov	x9, #0x0                ; =0
1000d58e0: 5280008a    	mov	w10, #0x4               ; =4
1000d58e4: 1400000d    	b	0x1000d5918 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2c8>
1000d58e8: aa0203e8    	mov	x8, x2
1000d58ec: 14000004    	b	0x1000d58fc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2ac>
1000d58f0: aa0503e9    	mov	x9, x5
1000d58f4: 14000009    	b	0x1000d5918 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x2c8>
1000d58f8: aa0603e8    	mov	x8, x6
1000d58fc: 900004c3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d5900: 91326063    	add	x3, x3, #0xc98
1000d5904: aa0803e0    	mov	x0, x8
1000d5908: aa0103e2    	mov	x2, x1
1000d590c: aa0903e1    	mov	x1, x9
1000d5910: 94014a23    	bl	0x10012819c <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d5914: aa0703e9    	mov	x9, x7
1000d5918: 900004c3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d591c: 9132c063    	add	x3, x3, #0xcb0
1000d5920: aa0903e0    	mov	x0, x9
1000d5924: aa0a03e1    	mov	x1, x10
1000d5928: aa0403e2    	mov	x2, x4
1000d592c: 94014a1c    	bl	0x10012819c <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d5930: aa0303f4    	mov	x20, x3
1000d5934: aa0003f3    	mov	x19, x0
1000d5938: aa0503f5    	mov	x21, x5
1000d593c: aa0203f7    	mov	x23, x2
1000d5940: aa0403f6    	mov	x22, x4
1000d5944: aa0103f8    	mov	x24, x1
1000d5948: 94013811    	bl	0x10012398c <__RNvNtNtNtCsfrLY33Z0RM3_8archmage6tokens9generated3arm11neon_detect>
1000d594c: aa1803e1    	mov	x1, x24
1000d5950: aa1603e4    	mov	x4, x22
1000d5954: aa1703e2    	mov	x2, x23
1000d5958: aa1503e5    	mov	x5, x21
1000d595c: aa1403e3    	mov	x3, x20
1000d5960: aa0003e8    	mov	x8, x0
1000d5964: aa1303e0    	mov	x0, x19
1000d5968: 35ffe908    	cbnz	w8, 0x1000d5688 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x38>
1000d596c: b4001da1    	cbz	x1, 0x1000d5d20 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6d0>
1000d5970: b4001dc4    	cbz	x4, 0x1000d5d28 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6d8>
1000d5974: f100043f    	cmp	x1, #0x1
1000d5978: 54001de0    	b.eq	0x1000d5d34 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6e4>
1000d597c: f100049f    	cmp	x4, #0x1
1000d5980: 54001de0    	b.eq	0x1000d5d3c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6ec>
1000d5984: f1000c3f    	cmp	x1, #0x3
1000d5988: 54001e03    	b.lo	0x1000d5d48 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6f8>
1000d598c: f1000c9f    	cmp	x4, #0x3
1000d5990: 54001e03    	b.lo	0x1000d5d50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x700>
1000d5994: f1000c3f    	cmp	x1, #0x3
1000d5998: 54001e20    	b.eq	0x1000d5d5c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x70c>
1000d599c: f1000c9f    	cmp	x4, #0x3
1000d59a0: 54001e20    	b.eq	0x1000d5d64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x714>
1000d59a4: eb01005f    	cmp	x2, x1
1000d59a8: 54001e42    	b.hs	0x1000d5d70 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x720>
1000d59ac: eb0400bf    	cmp	x5, x4
1000d59b0: 54001e42    	b.hs	0x1000d5d78 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x728>
1000d59b4: 9100044c    	add	x12, x2, #0x1
1000d59b8: eb01019f    	cmp	x12, x1
1000d59bc: 54001e42    	b.hs	0x1000d5d84 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x734>
1000d59c0: 910004ad    	add	x13, x5, #0x1
1000d59c4: eb0401bf    	cmp	x13, x4
1000d59c8: 54001e22    	b.hs	0x1000d5d8c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x73c>
1000d59cc: 91000848    	add	x8, x2, #0x2
1000d59d0: eb01011f    	cmp	x8, x1
1000d59d4: 54001e22    	b.hs	0x1000d5d98 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x748>
1000d59d8: 910008a9    	add	x9, x5, #0x2
1000d59dc: eb04013f    	cmp	x9, x4
1000d59e0: 54001e02    	b.hs	0x1000d5da0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x750>
1000d59e4: 91000c4e    	add	x14, x2, #0x3
1000d59e8: eb0101df    	cmp	x14, x1
1000d59ec: 54001e02    	b.hs	0x1000d5dac <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x75c>
1000d59f0: 91000caf    	add	x15, x5, #0x3
1000d59f4: eb0401ff    	cmp	x15, x4
1000d59f8: 54001de2    	b.hs	0x1000d5db4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x764>
1000d59fc: d37ff84a    	lsl	x10, x2, #1
1000d5a00: eb01015f    	cmp	x10, x1
1000d5a04: 54001de2    	b.hs	0x1000d5dc0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x770>
1000d5a08: d37ff8ab    	lsl	x11, x5, #1
1000d5a0c: eb04017f    	cmp	x11, x4
1000d5a10: 54001dc2    	b.hs	0x1000d5dc8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x778>
1000d5a14: b2400153    	orr	x19, x10, #0x1
1000d5a18: eb01027f    	cmp	x19, x1
1000d5a1c: 54001dc2    	b.hs	0x1000d5dd4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x784>
1000d5a20: b2400174    	orr	x20, x11, #0x1
1000d5a24: eb04029f    	cmp	x20, x4
1000d5a28: 54001da2    	b.hs	0x1000d5ddc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x78c>
1000d5a2c: 91000950    	add	x16, x10, #0x2
1000d5a30: eb01021f    	cmp	x16, x1
1000d5a34: 54001da2    	b.hs	0x1000d5de8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x798>
1000d5a38: 91000971    	add	x17, x11, #0x2
1000d5a3c: eb04023f    	cmp	x17, x4
1000d5a40: 54001d82    	b.hs	0x1000d5df0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7a0>
1000d5a44: 91000d55    	add	x21, x10, #0x3
1000d5a48: eb0102bf    	cmp	x21, x1
1000d5a4c: 54001d82    	b.hs	0x1000d5dfc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7ac>
1000d5a50: 91000d77    	add	x23, x11, #0x3
1000d5a54: eb0402ff    	cmp	x23, x4
1000d5a58: 54001d62    	b.hs	0x1000d5e04 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7b4>
1000d5a5c: 8b020446    	add	x6, x2, x2, lsl #1
1000d5a60: eb0100df    	cmp	x6, x1
1000d5a64: 54001d62    	b.hs	0x1000d5e10 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7c0>
1000d5a68: 8b0504a7    	add	x7, x5, x5, lsl #1
1000d5a6c: eb0400ff    	cmp	x7, x4
1000d5a70: 54001d42    	b.hs	0x1000d5e18 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7c8>
1000d5a74: 910004d8    	add	x24, x6, #0x1
1000d5a78: eb01031f    	cmp	x24, x1
1000d5a7c: 54001d42    	b.hs	0x1000d5e24 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7d4>
1000d5a80: 910004f9    	add	x25, x7, #0x1
1000d5a84: eb04033f    	cmp	x25, x4
1000d5a88: 54001d22    	b.hs	0x1000d5e2c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7dc>
1000d5a8c: 910008da    	add	x26, x6, #0x2
1000d5a90: eb01035f    	cmp	x26, x1
1000d5a94: 54001d22    	b.hs	0x1000d5e38 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7e8>
1000d5a98: 910008fb    	add	x27, x7, #0x2
1000d5a9c: eb04037f    	cmp	x27, x4
1000d5aa0: 54001d02    	b.hs	0x1000d5e40 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7f0>
1000d5aa4: 91000cd6    	add	x22, x6, #0x3
1000d5aa8: eb0102df    	cmp	x22, x1
1000d5aac: 54001d02    	b.hs	0x1000d5e4c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x7fc>
1000d5ab0: 91000ce1    	add	x1, x7, #0x3
1000d5ab4: eb04003f    	cmp	x1, x4
1000d5ab8: 54001d22    	b.hs	0x1000d5e5c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x80c>
1000d5abc: 387a6804    	ldrb	w4, [x0, x26]
1000d5ac0: 387b687a    	ldrb	w26, [x3, x27]
1000d5ac4: 4b1a0084    	sub	w4, w4, w26
1000d5ac8: 386c681a    	ldrb	w26, [x0, x12]
1000d5acc: 386d687b    	ldrb	w27, [x3, x13]
1000d5ad0: 3878680c    	ldrb	w12, [x0, x24]
1000d5ad4: 3879686d    	ldrb	w13, [x3, x25]
1000d5ad8: 4b0d018c    	sub	w12, w12, w13
1000d5adc: 3875680d    	ldrb	w13, [x0, x21]
1000d5ae0: 38776875    	ldrb	w21, [x3, x23]
1000d5ae4: 4b1501ad    	sub	w13, w13, w21
1000d5ae8: 39400015    	ldrb	w21, [x0]
1000d5aec: 38736813    	ldrb	w19, [x0, x19]
1000d5af0: 38746874    	ldrb	w20, [x3, x20]
1000d5af4: 4b140273    	sub	w19, w19, w20
1000d5af8: 39400414    	ldrb	w20, [x0, #0x1]
1000d5afc: 386e680e    	ldrb	w14, [x0, x14]
1000d5b00: 386f686f    	ldrb	w15, [x3, x15]
1000d5b04: 4b0f01cf    	sub	w15, w14, w15
1000d5b08: 39400c0e    	ldrb	w14, [x0, #0x3]
1000d5b0c: 4b1b0357    	sub	w23, w26, w27
1000d5b10: 39400c78    	ldrb	w24, [x3, #0x3]
1000d5b14: 4b1801ce    	sub	w14, w14, w24
1000d5b18: 39400478    	ldrb	w24, [x3, #0x1]
1000d5b1c: 4b180294    	sub	w20, w20, w24
1000d5b20: 39400078    	ldrb	w24, [x3]
1000d5b24: 4b1802b5    	sub	w21, w21, w24
1000d5b28: 0b150298    	add	w24, w20, w21
1000d5b2c: 4b1402b4    	sub	w20, w21, w20
1000d5b30: 38626802    	ldrb	w2, [x0, x2]
1000d5b34: 38656865    	ldrb	w5, [x3, x5]
1000d5b38: 38686815    	ldrb	w21, [x0, x8]
1000d5b3c: 38696879    	ldrb	w25, [x3, x9]
1000d5b40: 386a681a    	ldrb	w26, [x0, x10]
1000d5b44: 386b687b    	ldrb	w27, [x3, x11]
1000d5b48: 38706810    	ldrb	w16, [x0, x16]
1000d5b4c: 38716871    	ldrb	w17, [x3, x17]
1000d5b50: 38666806    	ldrb	w6, [x0, x6]
1000d5b54: 38766816    	ldrb	w22, [x0, x22]
1000d5b58: 39400808    	ldrb	w8, [x0, #0x2]
1000d5b5c: 38676860    	ldrb	w0, [x3, x7]
1000d5b60: 38616861    	ldrb	w1, [x3, x1]
1000d5b64: 39400869    	ldrb	w9, [x3, #0x2]
1000d5b68: 4b090108    	sub	w8, w8, w9
1000d5b6c: 0b0801c9    	add	w9, w14, w8
1000d5b70: 4b0e0108    	sub	w8, w8, w14
1000d5b74: 0b180123    	add	w3, w9, w24
1000d5b78: 4b09030b    	sub	w11, w24, w9
1000d5b7c: 0b140107    	add	w7, w8, w20
1000d5b80: 4b080288    	sub	w8, w20, w8
1000d5b84: 4b050049    	sub	w9, w2, w5
1000d5b88: 0b0902ee    	add	w14, w23, w9
1000d5b8c: 4b170129    	sub	w9, w9, w23
1000d5b90: 4b1902aa    	sub	w10, w21, w25
1000d5b94: 0b0a01e2    	add	w2, w15, w10
1000d5b98: 4b0f014a    	sub	w10, w10, w15
1000d5b9c: 4b1b034f    	sub	w15, w26, w27
1000d5ba0: 0b0f0265    	add	w5, w19, w15
1000d5ba4: 4b1301ef    	sub	w15, w15, w19
1000d5ba8: 4b110210    	sub	w16, w16, w17
1000d5bac: 0b1001b1    	add	w17, w13, w16
1000d5bb0: 4b0d020d    	sub	w13, w16, w13
1000d5bb4: 0b050230    	add	w16, w17, w5
1000d5bb8: 0b0f01b3    	add	w19, w13, w15
1000d5bbc: 4b1100b1    	sub	w17, w5, w17
1000d5bc0: 4b0d01ed    	sub	w13, w15, w13
1000d5bc4: 4b0000cf    	sub	w15, w6, w0
1000d5bc8: 0b0f0180    	add	w0, w12, w15
1000d5bcc: 4b0c01ec    	sub	w12, w15, w12
1000d5bd0: 4b0102cf    	sub	w15, w22, w1
1000d5bd4: 0b0401e1    	add	w1, w15, w4
1000d5bd8: 4b0f008f    	sub	w15, w4, w15
1000d5bdc: 0b0e0044    	add	w4, w2, w14
1000d5be0: 0b030085    	add	w5, w4, w3
1000d5be4: 4b040063    	sub	w3, w3, w4
1000d5be8: 0b000024    	add	w4, w1, w0
1000d5bec: 0b100086    	add	w6, w4, w16
1000d5bf0: 4b040210    	sub	w16, w16, w4
1000d5bf4: 2b0500c4    	adds	w4, w6, w5
1000d5bf8: 5a845484    	cneg	w4, w4, mi
1000d5bfc: 2b030214    	adds	w20, w16, w3
1000d5c00: 5a945694    	cneg	w20, w20, mi
1000d5c04: 6b0600a5    	subs	w5, w5, w6
1000d5c08: 5a8554a5    	cneg	w5, w5, mi
1000d5c0c: 0b140084    	add	w4, w4, w20
1000d5c10: 0b050084    	add	w4, w4, w5
1000d5c14: 6b100070    	subs	w16, w3, w16
1000d5c18: 5a905610    	cneg	w16, w16, mi
1000d5c1c: 0b090143    	add	w3, w10, w9
1000d5c20: 0b070065    	add	w5, w3, w7
1000d5c24: 4b0300e3    	sub	w3, w7, w3
1000d5c28: 0b0c01e6    	add	w6, w15, w12
1000d5c2c: 0b1300c7    	add	w7, w6, w19
1000d5c30: 4b060266    	sub	w6, w19, w6
1000d5c34: 2b0500f3    	adds	w19, w7, w5
1000d5c38: 5a935673    	cneg	w19, w19, mi
1000d5c3c: 2b0300d4    	adds	w20, w6, w3
1000d5c40: 5a945694    	cneg	w20, w20, mi
1000d5c44: 6b0700a5    	subs	w5, w5, w7
1000d5c48: 5a8554a5    	cneg	w5, w5, mi
1000d5c4c: 6b060063    	subs	w3, w3, w6
1000d5c50: 5a835463    	cneg	w3, w3, mi
1000d5c54: 4b0201ce    	sub	w14, w14, w2
1000d5c58: 0b0b01c2    	add	w2, w14, w11
1000d5c5c: 4b0e016b    	sub	w11, w11, w14
1000d5c60: 4b01000e    	sub	w14, w0, w1
1000d5c64: 0b1101c0    	add	w0, w14, w17
1000d5c68: 4b0e022e    	sub	w14, w17, w14
1000d5c6c: 2b020011    	adds	w17, w0, w2
1000d5c70: 5a915631    	cneg	w17, w17, mi
1000d5c74: 2b0b01c1    	adds	w1, w14, w11
1000d5c78: 5a815421    	cneg	w1, w1, mi
1000d5c7c: 6b000040    	subs	w0, w2, w0
1000d5c80: 5a805400    	cneg	w0, w0, mi
1000d5c84: 6b0e016b    	subs	w11, w11, w14
1000d5c88: 5a8b556b    	cneg	w11, w11, mi
1000d5c8c: 4b0a0129    	sub	w9, w9, w10
1000d5c90: 0b08012a    	add	w10, w9, w8
1000d5c94: 4b090108    	sub	w8, w8, w9
1000d5c98: 4b0f0189    	sub	w9, w12, w15
1000d5c9c: 0b0d012c    	add	w12, w9, w13
1000d5ca0: 4b0901a9    	sub	w9, w13, w9
1000d5ca4: 2b0a018d    	adds	w13, w12, w10
1000d5ca8: 5a8d55ad    	cneg	w13, w13, mi
1000d5cac: 2b08012e    	adds	w14, w9, w8
1000d5cb0: 5a8e55ce    	cneg	w14, w14, mi
1000d5cb4: 6b0c014a    	subs	w10, w10, w12
1000d5cb8: 5a8a554a    	cneg	w10, w10, mi
1000d5cbc: 6b090108    	subs	w8, w8, w9
1000d5cc0: 0b130209    	add	w9, w16, w19
1000d5cc4: 0b05028c    	add	w12, w20, w5
1000d5cc8: 0b0c0129    	add	w9, w9, w12
1000d5ccc: 0b03022c    	add	w12, w17, w3
1000d5cd0: 0b00002f    	add	w15, w1, w0
1000d5cd4: 0b0f018c    	add	w12, w12, w15
1000d5cd8: 0b0d016b    	add	w11, w11, w13
1000d5cdc: 0b0b018b    	add	w11, w12, w11
1000d5ce0: 5a885508    	cneg	w8, w8, mi
1000d5ce4: 0b0e014a    	add	w10, w10, w14
1000d5ce8: 0b080148    	add	w8, w10, w8
1000d5cec: 0b040108    	add	w8, w8, w4
1000d5cf0: 11000529    	add	w9, w9, #0x1
1000d5cf4: 12003d08    	and	w8, w8, #0xffff
1000d5cf8: 0b292108    	add	w8, w8, w9, uxth
1000d5cfc: 0b2b2108    	add	w8, w8, w11, uxth
1000d5d00: 53017d00    	lsr	w0, w8, #1
1000d5d04: a9457bfd    	ldp	x29, x30, [sp, #0x50]
1000d5d08: a9444ff4    	ldp	x20, x19, [sp, #0x40]
1000d5d0c: a94357f6    	ldp	x22, x21, [sp, #0x30]
1000d5d10: a9425ff8    	ldp	x24, x23, [sp, #0x20]
1000d5d14: a94167fa    	ldp	x26, x25, [sp, #0x10]
1000d5d18: a8c66ffc    	ldp	x28, x27, [sp], #0x60
1000d5d1c: d65f03c0    	ret
1000d5d20: d2800000    	mov	x0, #0x0                ; =0
1000d5d24: 1400004b    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d28: aa0403e1    	mov	x1, x4
1000d5d2c: d2800000    	mov	x0, #0x0                ; =0
1000d5d30: 1400004d    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d34: 52800020    	mov	w0, #0x1                ; =1
1000d5d38: 14000046    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d3c: aa0403e1    	mov	x1, x4
1000d5d40: 52800020    	mov	w0, #0x1                ; =1
1000d5d44: 14000048    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d48: 52800040    	mov	w0, #0x2                ; =2
1000d5d4c: 14000041    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d50: aa0403e1    	mov	x1, x4
1000d5d54: 52800040    	mov	w0, #0x2                ; =2
1000d5d58: 14000043    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d5c: 52800060    	mov	w0, #0x3                ; =3
1000d5d60: 1400003c    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d64: aa0403e1    	mov	x1, x4
1000d5d68: 52800060    	mov	w0, #0x3                ; =3
1000d5d6c: 1400003e    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d70: aa0203e0    	mov	x0, x2
1000d5d74: 14000037    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d78: aa0403e1    	mov	x1, x4
1000d5d7c: aa0503e0    	mov	x0, x5
1000d5d80: 14000039    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d84: aa0c03e0    	mov	x0, x12
1000d5d88: 14000032    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5d8c: aa0403e1    	mov	x1, x4
1000d5d90: aa0d03e0    	mov	x0, x13
1000d5d94: 14000034    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5d98: aa0803e0    	mov	x0, x8
1000d5d9c: 1400002d    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5da0: aa0403e1    	mov	x1, x4
1000d5da4: aa0903e0    	mov	x0, x9
1000d5da8: 1400002f    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5dac: aa0e03e0    	mov	x0, x14
1000d5db0: 14000028    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5db4: aa0403e1    	mov	x1, x4
1000d5db8: aa0f03e0    	mov	x0, x15
1000d5dbc: 1400002a    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5dc0: aa0a03e0    	mov	x0, x10
1000d5dc4: 14000023    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5dc8: aa0403e1    	mov	x1, x4
1000d5dcc: aa0b03e0    	mov	x0, x11
1000d5dd0: 14000025    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5dd4: aa1303e0    	mov	x0, x19
1000d5dd8: 1400001e    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5ddc: aa0403e1    	mov	x1, x4
1000d5de0: aa1403e0    	mov	x0, x20
1000d5de4: 14000020    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5de8: aa1003e0    	mov	x0, x16
1000d5dec: 14000019    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5df0: aa0403e1    	mov	x1, x4
1000d5df4: aa1103e0    	mov	x0, x17
1000d5df8: 1400001b    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5dfc: aa1503e0    	mov	x0, x21
1000d5e00: 14000014    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5e04: aa0403e1    	mov	x1, x4
1000d5e08: aa1703e0    	mov	x0, x23
1000d5e0c: 14000016    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5e10: aa0603e0    	mov	x0, x6
1000d5e14: 1400000f    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5e18: aa0403e1    	mov	x1, x4
1000d5e1c: aa0703e0    	mov	x0, x7
1000d5e20: 14000011    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5e24: aa1803e0    	mov	x0, x24
1000d5e28: 1400000a    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5e2c: aa0403e1    	mov	x1, x4
1000d5e30: aa1903e0    	mov	x0, x25
1000d5e34: 1400000c    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5e38: aa1a03e0    	mov	x0, x26
1000d5e3c: 14000005    	b	0x1000d5e50 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x800>
1000d5e40: aa0403e1    	mov	x1, x4
1000d5e44: aa1b03e0    	mov	x0, x27
1000d5e48: 14000007    	b	0x1000d5e64 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x814>
1000d5e4c: aa1603e0    	mov	x0, x22
1000d5e50: 900004c2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d5e54: 9130e042    	add	x2, x2, #0xc38
1000d5e58: 9401486b    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d5e5c: aa0103e0    	mov	x0, x1
1000d5e60: aa0403e1    	mov	x1, x4
1000d5e64: 900004c2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d5e68: 91314042    	add	x2, x2, #0xc50
1000d5e6c: 94014866    	bl	0x100128004 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
