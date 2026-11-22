import QtQuick
import app.chilpa
import app.chilpa.sources
import app.chilpa.kit
import app.chilpa.about
import "qrc:/qt/qml/app/chilpa/qml" as Shell

Item {
    id: test
    property var mainWindow
    property var state: ({})
    property int phase: 0
    property double albumId: 0
    property bool gaplessProof: GAPLESS
    property bool trimmed: false
    property string castTarget: CAST_TARGET
    property double seekTarget: SEEK_SECONDS
    property bool listenOnly: LISTEN_ONLY
    property double at: Date.now()
    property double refreshedAt: 0
    property double pausedAt: 0
    property string firstPath: ""
    property string pluginId: "io.github.vicrodh.qbz"
    property string pageKey: pluginId + " native:album:evz7usmruz35a"
    Component { id: shell; Shell.Main {} }
    PluginCenterModel { id: center }
    AlbumDetailModel { id: detail; albumId: test.albumId; remoteRev: Library.remoteRev + Library.remoteArtRev }
    TrackListModel { id: tracks; kind: "album"; albumId: detail.selectedId; albumPage: detail.tracksRev; order: "custom"; ascending: true }
    SettingsPage { id: audio; section: "audio" }
    function command(action, values) { center.pluginCommand(action, JSON.stringify(values)) }
    function next(number) { phase=number;at=Date.now();console.log("PROOF phase",number) }
    function advance() {
        const update=center.pluginsJson();if(update)state=JSON.parse(update)
        if(state.error)throw Error(state.error)
        if(state.busy||state.working||state.settings_loading_id)return
        if(phase===0){Onboarding.command("close");mainWindow=shell.createObject(null);command("install_local",{path:PACKAGE});next(1);return}
        if(phase===1&&state.approval){command("enable",{id:pluginId,permission_revision:state.approval.revision});next(2);return}
        if(phase===2){
            const row=(state.installed||[]).find(v=>v.id===pluginId)
            if(!row?.enabled)return
            if(row.problem)throw Error(row.problem)
            command("settings",{id:pluginId});next(21);return
        }
        if(phase===21&&state.form){
            if(EXISTING_SESSION){
                if(state.form.status?.phase!=="ready")throw Error("Existing session was not restored")
                next(3);return
            }
            if(state.form.wizard?.step!==2)throw Error("Firefox detection failed")
            command("submit",{id:pluginId,submission:{revision:state.form.revision,action:"login",values:{}}});next(22);return
        }
        if(phase===22&&state.form?.status?.phase==="ready"){
            next(3);return
        }
        if(phase===22&&Date.now()-refreshedAt>2000){refreshedAt=Date.now();command("settings",{id:pluginId,refresh:true});return}
        if(phase===3&&state.form?.status?.phase==="ready"){
            test.albumId=Sources.referenceCard(pluginId,"album:evz7usmruz35a");next(4);return
        }
        if(phase===4){

            if(tracks.count<2)return
            if(Date.now()-at<BROWSE_SECONDS*1000)return
            if(castTarget){Cast.picker(true);Cast.act("discover","","");next(40);return}
            tracks.play(0);next(5);return
        }
        if(phase===40){
            Cast.refresh();const document=JSON.parse(Cast.snapshot)
            const target=(document.entries||[]).find(v=>v.act==="connect"&&v.protocol==="upnp"&&v.text===castTarget)
            if(target){Cast.act("connect",target.protocol,target.id);next(41)}
            return
        }
        if(phase===41){
            Cast.refresh();const document=JSON.parse(Cast.snapshot)
            if(document.error)throw Error(document.error)
            if(document.state==="Connected"){console.log("PROOF cast connected",document.name);Cast.picker(false);tracks.play(0);next(5)}
            return
        }
        if(phase===5&&gaplessProof){
            if(Player.playing&&!trimmed){for(let row=15;row>=2;--row)Player.removeQueued(row);trimmed=true;}
            if(trimmed&&Date.now()-at>1500&&!Player.playing){console.log("PROOF PASS gapless queue finished");Qt.exit(0)}
            return
        }
        if(phase===5&&Player.playing&&Player.position>=HOLD_SECONDS){
            console.log("PROOF playback",Player.position,Player.qualityLabel,Player.outputFormat,Player.bitPerfect,Player.device,Player.route)
            if(listenOnly){audio.stopForDiagnosis();next(10);return}
            Player.toggle();next(6);return
        }
        if(phase===6&&Player.paused){pausedAt=Player.position;next(7);return}
        if(phase===7&&Date.now()-at>1200){
            if(Math.abs(Player.position-pausedAt)>.3)throw Error("Pause did not hold position")
            Player.seek(seekTarget);Player.toggle();next(8);return
        }
        if(phase===8&&Player.position>=seekTarget+1&&Player.position<seekTarget+7){console.log("PROOF seek",Player.position);firstPath=Player.path;Player.next();next(9);return}
        if(phase===9&&Player.path!==firstPath&&Date.now()-at>2500&&Player.playing&&Player.position>=1&&Player.position<10){
            console.log("PROOF second track",Player.qualityLabel,Player.bitPerfect)
            audio.stopForDiagnosis();next(10);return
        }
        if(phase===10&&!Player.playing){console.log(listenOnly ? "PROOF PASS install permissions playback hold stop" : "PROOF PASS install permissions source playback pause seek next stop");Qt.exit(0)}
    }
    Timer { interval:100;running:true;repeat:true;onTriggered:{try{test.advance()}catch(e){console.error("PROOF FAIL",String(e));Qt.exit(1)}} }
    Timer { interval:160000;running:true;onTriggered:{console.error("PROOF FAIL timeout phase",test.phase);Qt.exit(1)} }
}
