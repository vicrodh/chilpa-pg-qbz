import QtQuick
import app.chilpa
import app.chilpa.sources
import app.chilpa.controls
import app.chilpa.about
import "qrc:/qt/qml/app/chilpa/qml" as Shell

Item {
    id: test
    property var window
    property var state: ({})
    property int stage: 0
    property int pageIndex: 0
    property double requestedAt: 0
    property string pluginId: "io.github.vicrodh.qbz"
    property var pages: ["discover:home", "discover:editors", "discover:foryou"]
    Component { id: shell; Shell.Main {} }
    PluginCenterModel { id: center }
    SearchModel { id: search; source: test.pluginId }
    SettingsPage { id: audio; section: "audio" }
    function command(action, values) { center.pluginCommand(action, JSON.stringify(values)) }
    function find(item, predicate) {
        if (predicate(item)) return item
        for (const child of (item.children || [])) { const found = find(child, predicate); if (found) return found }
        return null
    }
    function page(reference) {
        const key = pluginId + " native:" + reference
        const document = JSON.parse(Sources.page(key, Library.remoteRev) || "{}")
        if (document.error) throw Error("Page " + reference + ": " + document.error)
        return document
    }
    function advance() {
        const update = center.pluginsJson(); if (update) state = JSON.parse(update)
        if (state.error) throw Error(state.error)
        if (state.busy || state.working || state.settings_loading_id) return
        if (stage === 0) {
            Onboarding.command("close"); window = shell.createObject(null)
            command("install_local", {path: PACKAGE}); stage = 1; return
        }
        if (stage === 1 && state.approval) {
            command("enable", {id:pluginId,permission_revision:state.approval.revision}); stage=2; return
        }
        if (stage === 2 && (state.installed || []).some(row=>row.id===pluginId && row.enabled)) {
            command("settings", {id:pluginId}); stage=3; return
        }
        if (stage === 3 && state.form) {
            Navigation.goKey("source", pluginId)
            Sources.page(pluginId + " native:discover:home", Library.remoteRev)
            stage=30; return
        }
        if (stage === 30) {
            const locked = JSON.parse(Sources.page(pluginId + " native:discover:home", Library.remoteRev) || "{}")
            if (!locked.error) return
            console.log("CATALOG signed-out page requires account setup")
            command("submit", {id:pluginId,submission:{revision:state.form.revision,action:"login",values:{}}})
            stage=4; return
        }
        if (stage === 4) {
            if (state.form?.status?.phase === "ready") {
                stage=5; return
            }
            if (Date.now()-requestedAt>2000) { requestedAt=Date.now();command("settings",{id:pluginId,refresh:true}) }
            return
        }
        if (stage === 5) {
            const button = find(window.contentItem, item=>item.objectName==="sourceParent" && item.text==="Qobuz")
            if (!button) return
            button.clicked(); console.log("CATALOG sidebar Qobuz activated"); stage=6; return
        }
        if (stage === 6) {
            const home = page("discover:home")
            if (!(home.shelves || []).some(shelf=>(shelf.items || []).length)) return
            console.log("CATALOG populated discovery")
            window.contentItem.grabToImage(result=>result.saveToFile(SCREENSHOT))
            Navigation.goKey("search", "Ummagumma"); search.text="Ummagumma"; stage=7; return
        }
        if (stage === 7) {
            search.refresh()
            if (search.count === 0) return
            const rows = JSON.parse(search.rowsJson())
            if (!rows.some(row=>row.sourceName==="Qobuz")) throw Error("Missing provider search result")
            console.log("CATALOG search results", search.count)
            stage=8; return
        }
        if (stage === 8) {
            if (pageIndex === pages.length) { stage=9; return }
            const reference = pages[pageIndex]
            Navigation.goKey("source", pluginId + " native:" + reference)
            const document = page(reference)
            if (document.loading || document.pending || Object.keys(document).length===0) return
            if (!document.shelves && !document.items && !document.entries && !document.header && !document.layout) return
            console.log("CATALOG page", reference)
            pageIndex++; return
        }
        if (stage === 9) {
            search.refresh()
            const rows=JSON.parse(search.rowsJson())
            const album=rows.findIndex(row=>String(row.title).toLowerCase().includes("ummagumma") && row.albumId)
            if (album<0) throw Error("No playable search album")
            search.act(album,"play"); stage=10; return
        }
        if (stage === 10 && Player.playing && Player.position>=2) {
            console.log("CATALOG playback from search",Player.qualityLabel,Player.bitPerfect)
            audio.stopForDiagnosis(); stage=11; return
        }
        if (stage === 11 && !Player.playing) { console.log("PROOF PASS sidebar discovery search playback");Qt.exit(0) }
    }
    Timer { interval:200;running:true;repeat:true;onTriggered:{try{test.advance()}catch(error){console.error("PROOF FAIL",String(error));Qt.exit(1)}} }
    Timer { interval:10000;running:true;repeat:true;onTriggered:console.log("CATALOG progress",test.stage,test.pageIndex) }
    Timer { interval:160000;running:true;onTriggered:{console.error("PROOF FAIL timeout",test.stage,test.pageIndex);Qt.exit(1)} }
}
