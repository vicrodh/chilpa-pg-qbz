(() => {
  const LIMIT = 32 * 1024 * 1024;
  const CHUNK = 1024 * 1024;
  const state = { id: '', queue: [], bytes: 0, total: 0, ended: false, error: '', head: 0 };
  function store() {
    const root = document.getElementById('root'), stack = [], seen = new Set();
    if (!root) return null;
    if (root._reactRootContainer?._internalRoot) stack.push(root._reactRootContainer._internalRoot.current);
    Object.keys(root).filter(k => /^__react(Container|Fiber|InternalInstance)/.test(k)).forEach(k => stack.push(root[k]));
    while (stack.length && seen.size < 80000) {
      const f = stack.pop(); if (!f || seen.has(f)) continue; seen.add(f);
      for (const v of [f.memoizedProps?.store, f.memoizedProps?.value])
        if (v && typeof v.getState === 'function' && typeof v.dispatch === 'function') return v;
      if (f.child) stack.push(f.child); if (f.sibling) stack.push(f.sibling);
    }
    return null;
  }
  function current() { return String(store()?.getState().player?.currentTrack?.id || ''); }
  function queueVersion() {
    const selector = (window.__zbqMods || []).find(module => typeof module.getCurrentPlayqueueVersion === 'function');
    if (!selector) throw Error('Official queue version is unavailable');
    return JSON.stringify(selector.getCurrentPlayqueueVersion(store().getState()));
  }
  const add = MediaSource.prototype.addSourceBuffer;
  MediaSource.prototype.addSourceBuffer = function(mime) {
    const buffer = add.apply(this, arguments);
    if (/^audio\//i.test(mime) && state.id) {
      buffer.__relayTrack = state.id; this.__relayTrack = state.id;
      state.source=this;state.buffer=buffer;
    }
    return buffer;
  };
  const append = SourceBuffer.prototype.appendBuffer;
  SourceBuffer.prototype.appendBuffer = function(data) {
    const result = append.apply(this, arguments);
    if (state.id && this === state.buffer && this.__relayTrack === state.id &&
        current() === state.id && queueVersion() !== state.previousQueue && !state.error) {
      if(!state.format){
        const bytes=data instanceof ArrayBuffer?new Uint8Array(data):new Uint8Array(data.buffer,data.byteOffset,data.byteLength);
        for(let i=4;i+46<=bytes.length;i++)if(bytes[i]===100&&bytes[i+1]===102&&bytes[i+2]===76&&bytes[i+3]===97){
          const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength),start=i+12,high=view.getUint32(start+10);
          state.format={codec:'flac',sample_rate:high>>>12,channels:((high>>>9)&7)+1,bits:((high>>>4)&31)+1,
            frames:(high&15)*4294967296+view.getUint32(start+14),md5:Array.from(bytes.subarray(start+18,start+34),b=>b.toString(16).padStart(2,'0')).join('')};break;
        }
      }
      if (state.bytes + data.byteLength > LIMIT) state.error = 'Browser transfer queue exceeded 32 MiB';
      else {
        const copy = data instanceof ArrayBuffer ? new Uint8Array(data.slice(0))
          : new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
        state.queue.push(copy); state.bytes += copy.length; state.total += copy.length;
      }
    }
    return result;
  };
  const eos = MediaSource.prototype.endOfStream;
  MediaSource.prototype.endOfStream = function() {
    const result = eos.apply(this, arguments);
    if (state.id && this === state.source && this.__relayTrack === state.id) state.ended = true;
    return result;
  };
  // Fetching and MSE buffering remain the web application's work. The browser
  // must not start an audio output; the receiving host owns playback.
  HTMLMediaElement.prototype.play = function() { state.media=this;this.muted = true; return Promise.resolve(); };
  window.__relay = {
    ready() { return !!(window.__zbq?.ready() && store()?.getState().user?.token); },
    localSelected() { return store()?.getState().audioOutputs?.current === 'default-audio-output'; },
    rememberSession() {
      const account = store()?.getState().user;
      const storage = (window.__zbqMods || []).find(module => module.STORAGE_USER_KEY === 'localuser');
      const details = account?.infos || account;
      if (!account?.token || !details?.id || !storage) return false;
      const session = {...details, token: account.token, store: account.store,
        credentialId: details.credential?.id};
      const fields = ['id', 'name', 'initials', 'login', 'avatar', 'email', 'languageCode',
        'countryCode', 'token', 'store', 'partner', 'credentialId', 'credential', 'subscription'];
      const remembered = Object.fromEntries(fields.filter(field => field in session && session[field] !== undefined).map(field => [field, session[field]]));
      localStorage.setItem(storage.STORAGE_USER_KEY, JSON.stringify(remembered));
      return JSON.parse(localStorage.getItem(storage.STORAGE_USER_KEY)).token === account.token;
    },
    async account() {
      const token=store()?.getState().user?.token;
      if(!token)throw Error('Sign in in Firefox');
      const response=await fetch('https://www.qobuz.com/api.json/0.2/user/get',{headers:{'X-App-Id':'798273057','X-User-Auth-Token':token}});
      if(!response.ok)throw Error('Firefox account validation failed');
      const result=await response.json(),user=result.user||result;
      if(!user.id)throw Error('Account has no identity');
      return {app_id:'798273057',session:{user_auth_token:token,user_id:user.id,email:'',display_name:user.display_name||user.login||'Qobuz',
        subscription_label:user.credential?.label||user.subscription?.offer||'',entitlements:user.credential?.parameters||{},
        country_code:user.country_code||null,language_code:user.language_code||null}};
    },
    local() {
      const s = store(), o = s?.getState().audioOutputs;
      if (!o?.dictionnary?.['default-audio-output']) return false;
      if (o.current === 'default-audio-output') return true;
      const action = (window.__zbqMods || []).find(m => typeof m.selectAudioOutput === 'function');
      if (action) s.dispatch(action.selectAudioOutput({output:o.dictionnary['default-audio-output']}));
      return false;
    },
    async metadata(id) {
      const token = store()?.getState().user?.token;
      if (!token) throw Error('Sign in on the official page');
      const response = await fetch('https://www.qobuz.com/api.json/0.2/track/get?track_id=' + encodeURIComponent(id),
        {headers:{'X-App-Id':'798273057','X-User-Auth-Token':token}});
      if (!response.ok) throw Error('Track metadata unavailable');
      const t = await response.json();
      return {kind:'track',ref:'track:'+String(t.id),title:t.title,artist:t.performer?.name || t.album?.artist?.name || '',
        album:t.album?.title || '',duration_ms:Math.round((t.duration || 0)*1000),available:t.streamable !== false};
    },
    async prepare(id,quality=27) {
      // Validate metadata while the official player starts fetching media.
      // The host releases no audio until both results have been checked.
      const [metadata] = await Promise.all([
        this.metadata(id),
        Promise.resolve().then(() => { this.begin(id,quality); window.__zbq.seek(0); })
      ]);
      return metadata;
    },
    begin(id,quality=27) {
      state.previousQueue=queueVersion();
      state.id=String(id);state.queue=[];state.bytes=0;state.total=0;state.ended=false;state.error='';state.head=0;state.format=null;state.source=null;state.buffer=null;
      window.__zbq.setMaxFormat(quality);
      window.__zbq.play([Number(id)],0,'track',String(id));
    },
    drain() {
      let encoded='';
      const length=Math.min(state.bytes,CHUNK);
      if(length){
        const batch=new Uint8Array(length);let written=0;
        while(written<length){
          const first=state.queue[0],count=Math.min(first.length-state.head,length-written);
          batch.set(first.subarray(state.head,state.head+count),written);
          written+=count;state.head+=count;state.bytes-=count;
          if(state.head===first.length){state.queue.shift();state.head=0;}
        }
        if(typeof batch.toBase64==='function')encoded=batch.toBase64();
        else {
          const pieces=[];
          for(let i=0;i<batch.length;i+=8192)pieces.push(String.fromCharCode(...batch.subarray(i,i+8192)));
          encoded=btoa(pieces.join(''));
        }
      }
      const duration=state.source?.duration, ranges=state.buffer?.buffered;
      const complete=Number.isFinite(duration)&&duration>0&&ranges?.length&&ranges.end(ranges.length-1)>=duration-.001;
      if(!complete&&state.media&&ranges?.length&&!state.buffer.updating&&state.bytes<1024*1024){
        const end=ranges.end(ranges.length-1);
        if(end-state.media.currentTime>5){state.media.currentTime=Math.max(0,end-.5);}
      }
      return {data:encoded,queued:state.bytes,total:state.total,ended:(state.ended||complete)&&!state.bytes,error:state.error,format:state.format,progress:{cursor:state.media?.currentTime||0,buffered:ranges?.length?ranges.end(ranges.length-1):0,duration:Number.isFinite(duration)?duration:0}};
    },
    stop() { window.__zbq?.pause();state.id='';state.queue=[];state.bytes=0;state.head=0; }
  };
})();
