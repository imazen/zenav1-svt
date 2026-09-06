
rust/target/release/deps/kernel_tiers-9d59c30f0562a5e1:	file format mach-o arm64

Disassembly of section __TEXT,__text:

00000001000d4e9c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4>:
1000d4e9c: a9ba6ffc    	stp	x28, x27, [sp, #-0x60]!
1000d4ea0: a90167fa    	stp	x26, x25, [sp, #0x10]
1000d4ea4: a9025ff8    	stp	x24, x23, [sp, #0x20]
1000d4ea8: a90357f6    	stp	x22, x21, [sp, #0x30]
1000d4eac: a9044ff4    	stp	x20, x19, [sp, #0x40]
1000d4eb0: a9057bfd    	stp	x29, x30, [sp, #0x50]
1000d4eb4: 910143fd    	add	x29, sp, #0x50
1000d4eb8: b0000508    	adrp	x8, 0x100175000 <__RNvNCNKNvNtNtCs7mRY9FNn263_3std6thread9spawnhook11SPAWN_HOOKS0023___RUST_STD_INTERNAL_VAL$tlv$init>
1000d4ebc: 9101e108    	add	x8, x8, #0x78
1000d4ec0: 39400108    	ldrb	w8, [x8]
1000d4ec4: 7100051f    	cmp	w8, #0x1
1000d4ec8: 540010e0    	b.eq	0x1000d50e4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x248>
1000d4ecc: 7100091f    	cmp	w8, #0x2
1000d4ed0: 54000ec1    	b.ne	0x1000d50a8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x20c>
1000d4ed4: f1000c3f    	cmp	x1, #0x3
1000d4ed8: 54000ae9    	b.ls	0x1000d5034 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x198>
1000d4edc: f1000c9f    	cmp	x4, #0x3
1000d4ee0: 54000b29    	b.ls	0x1000d5044 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1a8>
1000d4ee4: 9100104a    	add	x10, x2, #0x4
1000d4ee8: b100145f    	cmn	x2, #0x5
1000d4eec: 54000b28    	b.hi	0x1000d5050 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1b4>
1000d4ef0: eb01015f    	cmp	x10, x1
1000d4ef4: 54000ae8    	b.hi	0x1000d5050 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1b4>
1000d4ef8: 910010aa    	add	x10, x5, #0x4
1000d4efc: b10014bf    	cmn	x5, #0x5
1000d4f00: 54000ae8    	b.hi	0x1000d505c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1c0>
1000d4f04: eb04015f    	cmp	x10, x4
1000d4f08: 54000aa8    	b.hi	0x1000d505c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1c0>
1000d4f0c: d37ff848    	lsl	x8, x2, #1
1000d4f10: 9100110a    	add	x10, x8, #0x4
1000d4f14: eb01015f    	cmp	x10, x1
1000d4f18: 54000a68    	b.hi	0x1000d5064 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1c8>
1000d4f1c: d37ff8a9    	lsl	x9, x5, #1
1000d4f20: 9100112a    	add	x10, x9, #0x4
1000d4f24: eb04015f    	cmp	x10, x4
1000d4f28: 54000b48    	b.hi	0x1000d5090 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1f4>
1000d4f2c: 8b02044b    	add	x11, x2, x2, lsl #1
1000d4f30: 9100116a    	add	x10, x11, #0x4
1000d4f34: b100157f    	cmn	x11, #0x5
1000d4f38: 540009a8    	b.hi	0x1000d506c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1d0>
1000d4f3c: eb01015f    	cmp	x10, x1
1000d4f40: 54000968    	b.hi	0x1000d506c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1d0>
1000d4f44: 8b0504ac    	add	x12, x5, x5, lsl #1
1000d4f48: 9100118a    	add	x10, x12, #0x4
1000d4f4c: b100159f    	cmn	x12, #0x5
1000d4f50: 540009e8    	b.hi	0x1000d508c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1f0>
1000d4f54: eb04015f    	cmp	x10, x4
1000d4f58: 540009a8    	b.hi	0x1000d508c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1f0>
1000d4f5c: b940000a    	ldr	w10, [x0]
1000d4f60: b862680d    	ldr	w13, [x0, x2]
1000d4f64: b865686e    	ldr	w14, [x3, x5]
1000d4f68: b940006f    	ldr	w15, [x3]
1000d4f6c: b8686808    	ldr	w8, [x0, x8]
1000d4f70: b8696869    	ldr	w9, [x3, x9]
1000d4f74: 9e670100    	fmov	d0, x8
1000d4f78: 9e670121    	fmov	d1, x9
1000d4f7c: 2e212000    	usubl.8h	v0, v0, v1
1000d4f80: 9e6701a1    	fmov	d1, x13
1000d4f84: 9e6701c2    	fmov	d2, x14
1000d4f88: 2e222021    	usubl.8h	v1, v1, v2
1000d4f8c: 9e670142    	fmov	d2, x10
1000d4f90: 9e6701e3    	fmov	d3, x15
1000d4f94: 2e232042    	usubl.8h	v2, v2, v3
1000d4f98: b86b6808    	ldr	w8, [x0, x11]
1000d4f9c: b86c6869    	ldr	w9, [x3, x12]
1000d4fa0: 9e670103    	fmov	d3, x8
1000d4fa4: 9e670124    	fmov	d4, x9
1000d4fa8: 2e242063    	usubl.8h	v3, v3, v4
1000d4fac: 0e628424    	add.4h	v4, v1, v2
1000d4fb0: 2e618441    	sub.4h	v1, v2, v1
1000d4fb4: 0e608462    	add.4h	v2, v3, v0
1000d4fb8: 2e638400    	sub.4h	v0, v0, v3
1000d4fbc: 0e648443    	add.4h	v3, v2, v4
1000d4fc0: 0e618405    	add.4h	v5, v0, v1
1000d4fc4: 2e628482    	sub.4h	v2, v4, v2
1000d4fc8: 2e608420    	sub.4h	v0, v1, v0
1000d4fcc: 0e452861    	trn1.4h	v1, v3, v5
1000d4fd0: 0e456863    	trn2.4h	v3, v3, v5
1000d4fd4: 0e402844    	trn1.4h	v4, v2, v0
1000d4fd8: 0e406840    	trn2.4h	v0, v2, v0
1000d4fdc: 0e843822    	zip1.2s	v2, v1, v4
1000d4fe0: 0e803865    	zip1.2s	v5, v3, v0
1000d4fe4: 0e847821    	zip2.2s	v1, v1, v4
1000d4fe8: 0e807860    	zip2.2s	v0, v3, v0
1000d4fec: 0e658443    	add.4h	v3, v2, v5
1000d4ff0: 2e658442    	sub.4h	v2, v2, v5
1000d4ff4: 0e608424    	add.4h	v4, v1, v0
1000d4ff8: 2e608420    	sub.4h	v0, v1, v0
1000d4ffc: 0e648461    	add.4h	v1, v3, v4
1000d5000: 0e608445    	add.4h	v5, v2, v0
1000d5004: 2e648463    	sub.4h	v3, v3, v4
1000d5008: 2e608440    	sub.4h	v0, v2, v0
1000d500c: 0e60b821    	abs.4h	v1, v1
1000d5010: 0e60b8a2    	abs.4h	v2, v5
1000d5014: 2e620021    	uaddl.4s	v1, v1, v2
1000d5018: 6f00e402    	movi.2d	v2, #0000000000000000
1000d501c: 0e625061    	sabal.4s	v1, v3, v2
1000d5020: 0e625001    	sabal.4s	v1, v0, v2
1000d5024: 4eb1b820    	addv.4s	s0, v1
1000d5028: 1e260008    	fmov	w8, s0
1000d502c: 11000508    	add	w8, w8, #0x1
1000d5030: 14000112    	b	0x1000d5478 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x5dc>
1000d5034: aa0103e9    	mov	x9, x1
1000d5038: d2800008    	mov	x8, #0x0                ; =0
1000d503c: 5280008a    	mov	w10, #0x4               ; =4
1000d5040: 1400000d    	b	0x1000d5074 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1d8>
1000d5044: d2800009    	mov	x9, #0x0                ; =0
1000d5048: 5280008a    	mov	w10, #0x4               ; =4
1000d504c: 14000011    	b	0x1000d5090 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1f4>
1000d5050: aa0103e9    	mov	x9, x1
1000d5054: aa0203e8    	mov	x8, x2
1000d5058: 14000007    	b	0x1000d5074 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1d8>
1000d505c: aa0503e9    	mov	x9, x5
1000d5060: 1400000c    	b	0x1000d5090 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1f4>
1000d5064: aa0103e9    	mov	x9, x1
1000d5068: 14000003    	b	0x1000d5074 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x1d8>
1000d506c: aa0103e9    	mov	x9, x1
1000d5070: aa0b03e8    	mov	x8, x11
1000d5074: 900004c3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d5078: 912ea063    	add	x3, x3, #0xba8
1000d507c: aa0803e0    	mov	x0, x8
1000d5080: aa0a03e1    	mov	x1, x10
1000d5084: aa0903e2    	mov	x2, x9
1000d5088: 94014c52    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d508c: aa0c03e9    	mov	x9, x12
1000d5090: 900004c3    	adrp	x3, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d5094: 912f0063    	add	x3, x3, #0xbc0
1000d5098: aa0903e0    	mov	x0, x9
1000d509c: aa0a03e1    	mov	x1, x10
1000d50a0: aa0403e2    	mov	x2, x4
1000d50a4: 94014c4b    	bl	0x1001281d0 <__RNvNtNtCslWxY2MhVcag_4core5slice5index16slice_index_fail>
1000d50a8: aa0503f4    	mov	x20, x5
1000d50ac: aa0203f7    	mov	x23, x2
1000d50b0: aa0303f5    	mov	x21, x3
1000d50b4: aa0003f3    	mov	x19, x0
1000d50b8: aa0403f6    	mov	x22, x4
1000d50bc: aa0103f8    	mov	x24, x1
1000d50c0: 94013a40    	bl	0x1001239c0 <__RNvNtNtNtCsfrLY33Z0RM3_8archmage6tokens9generated3arm11neon_detect>
1000d50c4: aa1803e1    	mov	x1, x24
1000d50c8: aa1603e4    	mov	x4, x22
1000d50cc: aa1503e3    	mov	x3, x21
1000d50d0: aa1703e2    	mov	x2, x23
1000d50d4: aa1403e5    	mov	x5, x20
1000d50d8: aa0003e8    	mov	x8, x0
1000d50dc: aa1303e0    	mov	x0, x19
1000d50e0: 35ffefa8    	cbnz	w8, 0x1000d4ed4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x38>
1000d50e4: b4001da1    	cbz	x1, 0x1000d5498 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x5fc>
1000d50e8: b4001dc4    	cbz	x4, 0x1000d54a0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x604>
1000d50ec: f100043f    	cmp	x1, #0x1
1000d50f0: 54001de0    	b.eq	0x1000d54ac <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x610>
1000d50f4: f100049f    	cmp	x4, #0x1
1000d50f8: 54001de0    	b.eq	0x1000d54b4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x618>
1000d50fc: f1000c3f    	cmp	x1, #0x3
1000d5100: 54001e03    	b.lo	0x1000d54c0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x624>
1000d5104: f1000c9f    	cmp	x4, #0x3
1000d5108: 54001e03    	b.lo	0x1000d54c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x62c>
1000d510c: f1000c3f    	cmp	x1, #0x3
1000d5110: 54001e20    	b.eq	0x1000d54d4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x638>
1000d5114: f1000c9f    	cmp	x4, #0x3
1000d5118: 54001e20    	b.eq	0x1000d54dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x640>
1000d511c: eb01005f    	cmp	x2, x1
1000d5120: 54001e42    	b.hs	0x1000d54e8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x64c>
1000d5124: eb0400bf    	cmp	x5, x4
1000d5128: 54001e42    	b.hs	0x1000d54f0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x654>
1000d512c: 9100044c    	add	x12, x2, #0x1
1000d5130: eb01019f    	cmp	x12, x1
1000d5134: 54001e42    	b.hs	0x1000d54fc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x660>
1000d5138: 910004ad    	add	x13, x5, #0x1
1000d513c: eb0401bf    	cmp	x13, x4
1000d5140: 54001e22    	b.hs	0x1000d5504 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x668>
1000d5144: 91000848    	add	x8, x2, #0x2
1000d5148: eb01011f    	cmp	x8, x1
1000d514c: 54001e22    	b.hs	0x1000d5510 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x674>
1000d5150: 910008a9    	add	x9, x5, #0x2
1000d5154: eb04013f    	cmp	x9, x4
1000d5158: 54001e02    	b.hs	0x1000d5518 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x67c>
1000d515c: 91000c4e    	add	x14, x2, #0x3
1000d5160: eb0101df    	cmp	x14, x1
1000d5164: 54001e02    	b.hs	0x1000d5524 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x688>
1000d5168: 91000caf    	add	x15, x5, #0x3
1000d516c: eb0401ff    	cmp	x15, x4
1000d5170: 54001de2    	b.hs	0x1000d552c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x690>
1000d5174: d37ff84a    	lsl	x10, x2, #1
1000d5178: eb01015f    	cmp	x10, x1
1000d517c: 54001de2    	b.hs	0x1000d5538 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x69c>
1000d5180: d37ff8ab    	lsl	x11, x5, #1
1000d5184: eb04017f    	cmp	x11, x4
1000d5188: 54001dc2    	b.hs	0x1000d5540 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6a4>
1000d518c: b2400153    	orr	x19, x10, #0x1
1000d5190: eb01027f    	cmp	x19, x1
1000d5194: 54001dc2    	b.hs	0x1000d554c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6b0>
1000d5198: b2400174    	orr	x20, x11, #0x1
1000d519c: eb04029f    	cmp	x20, x4
1000d51a0: 54001da2    	b.hs	0x1000d5554 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6b8>
1000d51a4: 91000950    	add	x16, x10, #0x2
1000d51a8: eb01021f    	cmp	x16, x1
1000d51ac: 54001da2    	b.hs	0x1000d5560 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6c4>
1000d51b0: 91000971    	add	x17, x11, #0x2
1000d51b4: eb04023f    	cmp	x17, x4
1000d51b8: 54001d82    	b.hs	0x1000d5568 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6cc>
1000d51bc: 91000d55    	add	x21, x10, #0x3
1000d51c0: eb0102bf    	cmp	x21, x1
1000d51c4: 54001d82    	b.hs	0x1000d5574 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6d8>
1000d51c8: 91000d77    	add	x23, x11, #0x3
1000d51cc: eb0402ff    	cmp	x23, x4
1000d51d0: 54001d62    	b.hs	0x1000d557c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6e0>
1000d51d4: 8b020446    	add	x6, x2, x2, lsl #1
1000d51d8: eb0100df    	cmp	x6, x1
1000d51dc: 54001d62    	b.hs	0x1000d5588 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6ec>
1000d51e0: 8b0504a7    	add	x7, x5, x5, lsl #1
1000d51e4: eb0400ff    	cmp	x7, x4
1000d51e8: 54001d42    	b.hs	0x1000d5590 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x6f4>
1000d51ec: 910004d8    	add	x24, x6, #0x1
1000d51f0: eb01031f    	cmp	x24, x1
1000d51f4: 54001d42    	b.hs	0x1000d559c <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x700>
1000d51f8: 910004f9    	add	x25, x7, #0x1
1000d51fc: eb04033f    	cmp	x25, x4
1000d5200: 54001d22    	b.hs	0x1000d55a4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x708>
1000d5204: 910008da    	add	x26, x6, #0x2
1000d5208: eb01035f    	cmp	x26, x1
1000d520c: 54001d22    	b.hs	0x1000d55b0 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x714>
1000d5210: 910008fb    	add	x27, x7, #0x2
1000d5214: eb04037f    	cmp	x27, x4
1000d5218: 54001d02    	b.hs	0x1000d55b8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x71c>
1000d521c: 91000cd6    	add	x22, x6, #0x3
1000d5220: eb0102df    	cmp	x22, x1
1000d5224: 54001d02    	b.hs	0x1000d55c4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x728>
1000d5228: 91000ce1    	add	x1, x7, #0x3
1000d522c: eb04003f    	cmp	x1, x4
1000d5230: 54001d22    	b.hs	0x1000d55d4 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x738>
1000d5234: 387a6804    	ldrb	w4, [x0, x26]
1000d5238: 387b687a    	ldrb	w26, [x3, x27]
1000d523c: 4b1a0084    	sub	w4, w4, w26
1000d5240: 386c681a    	ldrb	w26, [x0, x12]
1000d5244: 386d687b    	ldrb	w27, [x3, x13]
1000d5248: 3878680c    	ldrb	w12, [x0, x24]
1000d524c: 3879686d    	ldrb	w13, [x3, x25]
1000d5250: 4b0d018c    	sub	w12, w12, w13
1000d5254: 3875680d    	ldrb	w13, [x0, x21]
1000d5258: 38776875    	ldrb	w21, [x3, x23]
1000d525c: 4b1501ad    	sub	w13, w13, w21
1000d5260: 39400015    	ldrb	w21, [x0]
1000d5264: 38736813    	ldrb	w19, [x0, x19]
1000d5268: 38746874    	ldrb	w20, [x3, x20]
1000d526c: 4b140273    	sub	w19, w19, w20
1000d5270: 39400414    	ldrb	w20, [x0, #0x1]
1000d5274: 386e680e    	ldrb	w14, [x0, x14]
1000d5278: 386f686f    	ldrb	w15, [x3, x15]
1000d527c: 4b0f01cf    	sub	w15, w14, w15
1000d5280: 39400c0e    	ldrb	w14, [x0, #0x3]
1000d5284: 4b1b0357    	sub	w23, w26, w27
1000d5288: 39400c78    	ldrb	w24, [x3, #0x3]
1000d528c: 4b1801ce    	sub	w14, w14, w24
1000d5290: 39400478    	ldrb	w24, [x3, #0x1]
1000d5294: 4b180294    	sub	w20, w20, w24
1000d5298: 39400078    	ldrb	w24, [x3]
1000d529c: 4b1802b5    	sub	w21, w21, w24
1000d52a0: 0b150298    	add	w24, w20, w21
1000d52a4: 4b1402b4    	sub	w20, w21, w20
1000d52a8: 38626802    	ldrb	w2, [x0, x2]
1000d52ac: 38656865    	ldrb	w5, [x3, x5]
1000d52b0: 38686815    	ldrb	w21, [x0, x8]
1000d52b4: 38696879    	ldrb	w25, [x3, x9]
1000d52b8: 386a681a    	ldrb	w26, [x0, x10]
1000d52bc: 386b687b    	ldrb	w27, [x3, x11]
1000d52c0: 38706810    	ldrb	w16, [x0, x16]
1000d52c4: 38716871    	ldrb	w17, [x3, x17]
1000d52c8: 38666806    	ldrb	w6, [x0, x6]
1000d52cc: 38766816    	ldrb	w22, [x0, x22]
1000d52d0: 39400808    	ldrb	w8, [x0, #0x2]
1000d52d4: 38676860    	ldrb	w0, [x3, x7]
1000d52d8: 38616861    	ldrb	w1, [x3, x1]
1000d52dc: 39400869    	ldrb	w9, [x3, #0x2]
1000d52e0: 4b090108    	sub	w8, w8, w9
1000d52e4: 0b0801c9    	add	w9, w14, w8
1000d52e8: 4b0e0108    	sub	w8, w8, w14
1000d52ec: 0b180123    	add	w3, w9, w24
1000d52f0: 4b09030b    	sub	w11, w24, w9
1000d52f4: 0b140107    	add	w7, w8, w20
1000d52f8: 4b080288    	sub	w8, w20, w8
1000d52fc: 4b050049    	sub	w9, w2, w5
1000d5300: 0b0902ee    	add	w14, w23, w9
1000d5304: 4b170129    	sub	w9, w9, w23
1000d5308: 4b1902aa    	sub	w10, w21, w25
1000d530c: 0b0a01e2    	add	w2, w15, w10
1000d5310: 4b0f014a    	sub	w10, w10, w15
1000d5314: 4b1b034f    	sub	w15, w26, w27
1000d5318: 0b0f0265    	add	w5, w19, w15
1000d531c: 4b1301ef    	sub	w15, w15, w19
1000d5320: 4b110210    	sub	w16, w16, w17
1000d5324: 0b1001b1    	add	w17, w13, w16
1000d5328: 4b0d020d    	sub	w13, w16, w13
1000d532c: 0b050230    	add	w16, w17, w5
1000d5330: 0b0f01b3    	add	w19, w13, w15
1000d5334: 4b1100b1    	sub	w17, w5, w17
1000d5338: 4b0d01ed    	sub	w13, w15, w13
1000d533c: 4b0000cf    	sub	w15, w6, w0
1000d5340: 0b0f0180    	add	w0, w12, w15
1000d5344: 4b0c01ec    	sub	w12, w15, w12
1000d5348: 4b0102cf    	sub	w15, w22, w1
1000d534c: 0b0401e1    	add	w1, w15, w4
1000d5350: 4b0f008f    	sub	w15, w4, w15
1000d5354: 0b0e0044    	add	w4, w2, w14
1000d5358: 0b030085    	add	w5, w4, w3
1000d535c: 4b040063    	sub	w3, w3, w4
1000d5360: 0b000024    	add	w4, w1, w0
1000d5364: 0b100086    	add	w6, w4, w16
1000d5368: 4b040210    	sub	w16, w16, w4
1000d536c: 2b0500c4    	adds	w4, w6, w5
1000d5370: 5a845484    	cneg	w4, w4, mi
1000d5374: 2b030214    	adds	w20, w16, w3
1000d5378: 5a945694    	cneg	w20, w20, mi
1000d537c: 6b0600a5    	subs	w5, w5, w6
1000d5380: 5a8554a5    	cneg	w5, w5, mi
1000d5384: 0b140084    	add	w4, w4, w20
1000d5388: 0b050084    	add	w4, w4, w5
1000d538c: 6b100070    	subs	w16, w3, w16
1000d5390: 5a905610    	cneg	w16, w16, mi
1000d5394: 0b090143    	add	w3, w10, w9
1000d5398: 0b070065    	add	w5, w3, w7
1000d539c: 4b0300e3    	sub	w3, w7, w3
1000d53a0: 0b0c01e6    	add	w6, w15, w12
1000d53a4: 0b1300c7    	add	w7, w6, w19
1000d53a8: 4b060266    	sub	w6, w19, w6
1000d53ac: 2b0500f3    	adds	w19, w7, w5
1000d53b0: 5a935673    	cneg	w19, w19, mi
1000d53b4: 2b0300d4    	adds	w20, w6, w3
1000d53b8: 5a945694    	cneg	w20, w20, mi
1000d53bc: 6b0700a5    	subs	w5, w5, w7
1000d53c0: 5a8554a5    	cneg	w5, w5, mi
1000d53c4: 6b060063    	subs	w3, w3, w6
1000d53c8: 5a835463    	cneg	w3, w3, mi
1000d53cc: 4b0201ce    	sub	w14, w14, w2
1000d53d0: 0b0b01c2    	add	w2, w14, w11
1000d53d4: 4b0e016b    	sub	w11, w11, w14
1000d53d8: 4b01000e    	sub	w14, w0, w1
1000d53dc: 0b1101c0    	add	w0, w14, w17
1000d53e0: 4b0e022e    	sub	w14, w17, w14
1000d53e4: 2b020011    	adds	w17, w0, w2
1000d53e8: 5a915631    	cneg	w17, w17, mi
1000d53ec: 2b0b01c1    	adds	w1, w14, w11
1000d53f0: 5a815421    	cneg	w1, w1, mi
1000d53f4: 6b000040    	subs	w0, w2, w0
1000d53f8: 5a805400    	cneg	w0, w0, mi
1000d53fc: 6b0e016b    	subs	w11, w11, w14
1000d5400: 5a8b556b    	cneg	w11, w11, mi
1000d5404: 4b0a0129    	sub	w9, w9, w10
1000d5408: 0b08012a    	add	w10, w9, w8
1000d540c: 4b090108    	sub	w8, w8, w9
1000d5410: 4b0f0189    	sub	w9, w12, w15
1000d5414: 0b0d012c    	add	w12, w9, w13
1000d5418: 4b0901a9    	sub	w9, w13, w9
1000d541c: 2b0a018d    	adds	w13, w12, w10
1000d5420: 5a8d55ad    	cneg	w13, w13, mi
1000d5424: 2b08012e    	adds	w14, w9, w8
1000d5428: 5a8e55ce    	cneg	w14, w14, mi
1000d542c: 6b0c014a    	subs	w10, w10, w12
1000d5430: 5a8a554a    	cneg	w10, w10, mi
1000d5434: 6b090108    	subs	w8, w8, w9
1000d5438: 0b130209    	add	w9, w16, w19
1000d543c: 0b05028c    	add	w12, w20, w5
1000d5440: 0b0c0129    	add	w9, w9, w12
1000d5444: 0b03022c    	add	w12, w17, w3
1000d5448: 0b00002f    	add	w15, w1, w0
1000d544c: 0b0f018c    	add	w12, w12, w15
1000d5450: 0b0d016b    	add	w11, w11, w13
1000d5454: 0b0b018b    	add	w11, w12, w11
1000d5458: 5a885508    	cneg	w8, w8, mi
1000d545c: 0b0e014a    	add	w10, w10, w14
1000d5460: 0b080148    	add	w8, w10, w8
1000d5464: 0b040108    	add	w8, w8, w4
1000d5468: 11000529    	add	w9, w9, #0x1
1000d546c: 12003d08    	and	w8, w8, #0xffff
1000d5470: 0b292108    	add	w8, w8, w9, uxth
1000d5474: 0b2b2108    	add	w8, w8, w11, uxth
1000d5478: 53017d00    	lsr	w0, w8, #1
1000d547c: a9457bfd    	ldp	x29, x30, [sp, #0x50]
1000d5480: a9444ff4    	ldp	x20, x19, [sp, #0x40]
1000d5484: a94357f6    	ldp	x22, x21, [sp, #0x30]
1000d5488: a9425ff8    	ldp	x24, x23, [sp, #0x20]
1000d548c: a94167fa    	ldp	x26, x25, [sp, #0x10]
1000d5490: a8c66ffc    	ldp	x28, x27, [sp], #0x60
1000d5494: d65f03c0    	ret
1000d5498: d2800000    	mov	x0, #0x0                ; =0
1000d549c: 1400004b    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d54a0: aa0403e1    	mov	x1, x4
1000d54a4: d2800000    	mov	x0, #0x0                ; =0
1000d54a8: 1400004d    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d54ac: 52800020    	mov	w0, #0x1                ; =1
1000d54b0: 14000046    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d54b4: aa0403e1    	mov	x1, x4
1000d54b8: 52800020    	mov	w0, #0x1                ; =1
1000d54bc: 14000048    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d54c0: 52800040    	mov	w0, #0x2                ; =2
1000d54c4: 14000041    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d54c8: aa0403e1    	mov	x1, x4
1000d54cc: 52800040    	mov	w0, #0x2                ; =2
1000d54d0: 14000043    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d54d4: 52800060    	mov	w0, #0x3                ; =3
1000d54d8: 1400003c    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d54dc: aa0403e1    	mov	x1, x4
1000d54e0: 52800060    	mov	w0, #0x3                ; =3
1000d54e4: 1400003e    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d54e8: aa0203e0    	mov	x0, x2
1000d54ec: 14000037    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d54f0: aa0403e1    	mov	x1, x4
1000d54f4: aa0503e0    	mov	x0, x5
1000d54f8: 14000039    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d54fc: aa0c03e0    	mov	x0, x12
1000d5500: 14000032    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5504: aa0403e1    	mov	x1, x4
1000d5508: aa0d03e0    	mov	x0, x13
1000d550c: 14000034    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5510: aa0803e0    	mov	x0, x8
1000d5514: 1400002d    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5518: aa0403e1    	mov	x1, x4
1000d551c: aa0903e0    	mov	x0, x9
1000d5520: 1400002f    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5524: aa0e03e0    	mov	x0, x14
1000d5528: 14000028    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d552c: aa0403e1    	mov	x1, x4
1000d5530: aa0f03e0    	mov	x0, x15
1000d5534: 1400002a    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5538: aa0a03e0    	mov	x0, x10
1000d553c: 14000023    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5540: aa0403e1    	mov	x1, x4
1000d5544: aa0b03e0    	mov	x0, x11
1000d5548: 14000025    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d554c: aa1303e0    	mov	x0, x19
1000d5550: 1400001e    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5554: aa0403e1    	mov	x1, x4
1000d5558: aa1403e0    	mov	x0, x20
1000d555c: 14000020    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5560: aa1003e0    	mov	x0, x16
1000d5564: 14000019    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5568: aa0403e1    	mov	x1, x4
1000d556c: aa1103e0    	mov	x0, x17
1000d5570: 1400001b    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5574: aa1503e0    	mov	x0, x21
1000d5578: 14000014    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d557c: aa0403e1    	mov	x1, x4
1000d5580: aa1703e0    	mov	x0, x23
1000d5584: 14000016    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d5588: aa0603e0    	mov	x0, x6
1000d558c: 1400000f    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d5590: aa0403e1    	mov	x1, x4
1000d5594: aa0703e0    	mov	x0, x7
1000d5598: 14000011    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d559c: aa1803e0    	mov	x0, x24
1000d55a0: 1400000a    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d55a4: aa0403e1    	mov	x1, x4
1000d55a8: aa1903e0    	mov	x0, x25
1000d55ac: 1400000c    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d55b0: aa1a03e0    	mov	x0, x26
1000d55b4: 14000005    	b	0x1000d55c8 <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x72c>
1000d55b8: aa0403e1    	mov	x1, x4
1000d55bc: aa1b03e0    	mov	x0, x27
1000d55c0: 14000007    	b	0x1000d55dc <__RNvNtCs9fmhdn4BMZD_10svtav1_dsp8hadamard8satd_4x4+0x740>
1000d55c4: aa1603e0    	mov	x0, x22
1000d55c8: 900004c2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d55cc: 912d2042    	add	x2, x2, #0xb48
1000d55d0: 94014a9a    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
1000d55d4: aa0103e0    	mov	x0, x1
1000d55d8: aa0403e1    	mov	x1, x4
1000d55dc: 900004c2    	adrp	x2, 0x10016d000 <_anon.3db9c0ad78b4a1f9064bd4503cd48a57.25+0xd70>
1000d55e0: 912d8042    	add	x2, x2, #0xb60
1000d55e4: 94014a95    	bl	0x100128038 <__RNvNtCslWxY2MhVcag_4core9panicking18panic_bounds_check>
