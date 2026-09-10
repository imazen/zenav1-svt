from pathlib import Path
import os, subprocess
root=Path('/home/lilith/work/zen/zenav1-svt/rust')
out=Path('/home/lilith/tmp/svt-tracking/tune-before'); out.mkdir(exist_ok=True)
for tune in (1,0):
 p=out/f'tune{tune}'
 env=dict(os.environ,SVTAV1_TUNE=str(tune),SVT_TUNE=str(tune),SVT_TRACE_OUT='/dev/null',SVT_BUILD_JOBS='4')
 for name,cmd in [('rust',[str(root/'tools/identity_run'),'gradient','72','88','40','8',str(p)]),('c',[str(root/'tools/capture_c_trace/capture_c_trace'),'72','88','40','8',str(p)+'.yuv',str(p)+'.c.obu','8'])]:
  with open(str(p)+'.'+name+'.log','w') as f:subprocess.run(cmd,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
 a=Path(str(p)+'.obu').read_bytes(); b=Path(str(p)+'.c.obu').read_bytes()
 print(f'tune={tune} rust={len(a)} C={len(b)} exact={a==b}',flush=True)
