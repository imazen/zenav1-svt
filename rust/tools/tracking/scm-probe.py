from pathlib import Path
import os, subprocess, sys
root=Path('/home/lilith/work/zen/zenav1-svt/rust')
out=Path('/home/lilith/tmp/svt-tracking')/sys.argv[1];out.mkdir(exist_ok=True)
for scm,content,preset in [(0,'screen',6),(1,'gradient',6),(1,'gradient',8)]:
 p=out/f'scm{scm}-{content}-p{preset}'
 env=dict(os.environ,SVTAV1_SCM=str(scm),SVT_SCM=str(scm),SVT_TRACE_OUT='/dev/null',SVT_BUILD_JOBS='4')
 for name,cmd in [('rust',[str(root/'tools/identity_run'),content,'72','88','40',str(preset),str(p)]),('c',[str(root/'tools/capture_c_trace/capture_c_trace'),'72','88','40',str(preset),str(p)+'.yuv',str(p)+'.c.obu','8'])]:
  with open(str(p)+'.'+name+'.log','w') as f:subprocess.run(cmd,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
 a=Path(str(p)+'.obu').read_bytes();b=Path(str(p)+'.c.obu').read_bytes()
 print(f'{p.name} rust={len(a)} C={len(b)} exact={a==b}',flush=True)
