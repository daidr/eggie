#!/usr/bin/env python3
"""Exercise real interactive shells through PTYs; no user shell configuration is modified."""
import os, pty, select, signal, time, re, tempfile, pathlib, subprocess, urllib.parse, fcntl, termios, struct
ROOT=pathlib.Path(__file__).resolve().parents[2]
class Shell:
    def __init__(self,argv,setup,cwd):
        self.program=argv[0]
        self.pid,self.fd=pty.fork()
        if self.pid==0:
            os.chdir(cwd);os.environ['HOME']=str(pathlib.Path(cwd,'test-home'));os.environ['XDG_CONFIG_HOME']=str(pathlib.Path(cwd,'test-home','.config'));os.environ['XDG_DATA_HOME']=str(pathlib.Path(cwd,'test-home','.local','share'));os.environ['TERM']='xterm-256color';os.environ.pop('NO_COLOR',None);os.execv(argv[0],argv)
        fcntl.ioctl(self.fd,termios.TIOCSWINSZ,struct.pack('HHHH',40,120,0,0))
        self.read(1);os.write(self.fd,(setup+'\n').encode());self.startup=self.read(4)
    def read(self,seconds=.2,native=False):
        data=b'';end=time.monotonic()+seconds
        while time.monotonic()<end:
            ready,_,_=select.select([self.fd],[],[],min(.05,end-time.monotonic()))
            if ready:
                try:
                    chunk=os.read(self.fd,65536);data+=chunk
                    if b'\x1b[0c' in chunk: os.write(self.fd,b'\x1b[?62;4;6;22c')
                    if b'\x1b[?u' in chunk: os.write(self.fd,b'\x1b[?0u')
                    if b'\x1b[>0q' in chunk: os.write(self.fd,b'\x1bP>|Eggie(0)\x1b\\')
                    if b'\x1b]11;?' in chunk: os.write(self.fd,b'\x1b]11;rgb:ffff/ffff/ffff\x1b\\')
                    if b'\x1b[6n' in chunk: os.write(self.fd,b'\x1b[1;1R')
                    if native and re.search(rb"\x1b\]1337;EggieCompletions=[^\x07]*\x07",data):break
                except OSError:break
        return data
    def complete(self,text,expected=None):
        os.write(self.fd,text.encode());self.read(.15);os.write(self.fd,b'\x18\x1f');data=self.read(8,native=True)
        reports=re.findall(rb'\x1b\]1337;EggieCompletions=([^\x07]*)\x07',data)
        assert reports,(text,shell.startup[-2000:],data[-3000:])
        fields=reports[-1].decode().split(';')
        assert urllib.parse.unquote(fields[0])==(text if expected is None else expected),(text,fields[:2])
        self.read(.15)
        return [urllib.parse.unquote(s).split("\t")[0] for s in fields[2:]]
    def apply(self,text,cursor,start,end,insert):
        packet='\x18\x1e'+urllib.parse.quote(text,safe='')+';'+str(cursor)+';'+str(start)+';'+str(end)+';'+urllib.parse.quote(insert,safe='')+('\0' if 'fish' in self.program else '\n')
        os.write(self.fd,packet.encode());self.read(.3)
    def clear(self):os.write(self.fd,b'\x03');self.read(.2)
    def close(self):
        os.close(self.fd)
        try:os.kill(self.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        os.waitpid(self.pid,0)
with tempfile.TemporaryDirectory(prefix='eggie-completion-') as tmp:
    pathlib.Path(tmp,'test-home').mkdir();pathlib.Path(tmp,'a file.txt').touch();pathlib.Path(tmp,'中文.txt').touch();pathlib.Path(tmp,'folder').mkdir();pathlib.Path(tmp,'folder','nested.txt').touch()
    subprocess.run(['git','init','-q',tmp],check=True)
    subprocess.run(['git','-C',tmp,'-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','test'],check=True)
    subprocess.run(['git','-C',tmp,'branch','test-feature'],check=True)
    configs=[('/bin/zsh',['/bin/zsh','-dfi'],f'source {ROOT}/crates/eggie-daemon/shell-integration/zsh/eggie-integration; eggie-integration; alias eggie_test_alias="echo hello"'),
      ('/opt/homebrew/bin/bash',['/opt/homebrew/bin/bash','--noprofile','--norc','-i'],f'source {ROOT}/crates/eggie-daemon/shell-integration/bash/eggie.bash; alias eggie_test_alias="echo hello"; complete -W "alpha beta" eggie_test_words'),
      ('/opt/homebrew/bin/fish',['/opt/homebrew/bin/fish','--no-config','-i'],f'source {ROOT}/crates/eggie-daemon/shell-integration/fish/vendor_conf.d/eggie.fish; alias eggie_test_alias "echo hello"')]
    for executable,argv,setup in configs:
        if not pathlib.Path(executable).exists() or (os.environ.get("SHELL_TEST_FILTER") and os.environ["SHELL_TEST_FILTER"] not in executable):continue
        shell=Shell(argv,setup,tmp)
        try:
            assert b"bind: cannot parse" not in shell.startup, shell.startup[-2000:]
            names=shell.complete('eggie_test_');assert 'eggie_test_alias' in names,(executable,names[:20]);shell.clear()
            if 'zsh' in executable:
                # Every redraw reports the real buffer, including keys that leave it unchanged.
                text=''
                for character in 'git ch':
                    text+=character
                    os.write(shell.fd,character.encode());data=shell.read(.08)
                    reports=re.findall(rb'\x1b\]1337;EggieInput=([^\x07]*)\x07',data)
                    assert reports,(text,data)
                    assert urllib.parse.unquote(reports[-1].decode().split(';',6)[6])==text
                os.write(shell.fd,b'\x1b[C');data=shell.read(.2)
                reports=re.findall(rb'\x1b\]1337;EggieInput=([^\x07]*)\x07',data)
                assert reports,('unchanged redraw did not acknowledge input',data)
                assert urllib.parse.unquote(reports[-1].decode().split(';',6)[6])==text
                shell.clear()
            if 'bash' in executable:
                assert 'alpha' in shell.complete('eggie_test_words al');shell.clear()
            names=shell.complete('cat a');assert any('a file.txt' in n for n in names),(executable,names);shell.clear()
            names=shell.complete('cat folder/n');assert any(n == 'folder/nested.txt' for n in names),(executable,names);shell.clear()
            names=shell.complete('cat 中');assert any('中文.txt' in n for n in names),(executable,names);shell.clear()
            if 'bash' not in executable:
                names=shell.complete('git checkout test-');assert 'test-feature' in names,(executable,names);shell.clear()
            shell.clear();shell.complete('cat 中')
            shell.apply('cat 中',5,4,5,'中文.txt')
            assert '中文.txt' in shell.complete('',expected='cat 中文.txt')
            shell.clear()
            shell.complete('cat a')
            # A response for an older edit must not delete freshly typed text.
            os.write(shell.fd,b'z');shell.read(.2)
            shell.apply('cat a',5,4,5,'a file.txt')
            shell.complete('',expected='cat az');shell.clear()
            # Replace a token with the cursor in its middle, preserving the next argument.
            os.write(shell.fd,b'cat aXX tail');shell.read(.2)
            os.write(shell.fd,b'\x1b[D'*8);shell.read(.2)
            shell.apply('cat aXX tail',4,4,7,'a\\ file.txt')
            shell.complete('',expected='cat a\\ file.txt tail');shell.clear()
            multiline='echo one\ncat a'
            os.write(shell.fd,b'\x1b[200~'+multiline.encode()+b'\x1b[201~');shell.read(.2)
            assert 'a file.txt' in shell.complete('',expected=multiline)
            shell.apply(multiline,len(multiline),len(multiline)-1,len(multiline),'a\\ file.txt')
            shell.complete('',expected='echo one\ncat a\\ file.txt');shell.clear()
            print(executable, 'aliases, paths, unicode, native candidates, stale and multiline edits: OK',flush=True)
        finally:shell.close()
