import QtQuick
import QtQuick.Window
import app.chilpa
import app.chilpa.sources
import app.chilpa.kit
import "qrc:/qt/qml/app/chilpa/sources/qml/Patterns"
import "qrc:/qt/qml/app/chilpa/kit/qml/Theme"

Window {
    id: test
    width: 760
    height: 720
    visible: true
    color: Theme.color.surfacePanel
    Rectangle { anchors.fill: parent; color: Theme.color.surfacePanel }
    property var state: ({})
    property int stage: 0
    property string pluginId: "io.github.vicrodh.qbz"
    property bool denied: DENIED
    PluginCenterModel { id: center }
        PluginSettingsForm {
            id: form
            x: 32; y: 32; width: parent.width - 64
            pluginId: test.pluginId
            pluginName: "Qobuz"
            document: test.state.form || ({})
            hostBusy: !!test.state.settings_loading_id
            enabled: !hostBusy
            onSubmitted: (action, values, scope) => test.command("submit", {id:scope.plugin,submission:{revision:scope.revision,action:action,values:values}})
            onRefreshed: test.command("settings", {id:test.pluginId,refresh:true})
        }
    function command(action, input) { center.pluginCommand(action, JSON.stringify(input)) }
    function click(name) {
        let found = null
        function walk(item) {
            if (item.objectName === name) found = item
            for (const child of item.children || []) walk(child)
        }
        walk(form)
        if (!found) throw Error("Wizard action missing: " + name)
        found.clicked()
    }
    function advance() {
        const update = center.pluginsJson()
        if (update) state = JSON.parse(update)
        if (state.error) throw Error(state.error)
        if (state.busy || state.working || state.settings_loading_id) return
        if (stage === 0) { command("install_local", {path:PACKAGE}); stage=1; return }
        if (stage === 1 && state.approval) {
            command("enable", {id:pluginId,permission_revision:state.approval.revision}); stage=2; return
        }
        if (stage === 2 && (state.installed || []).some(p => p.id === pluginId && p.enabled)) {
            command("settings",{id:pluginId}); stage=3; return
        }
        if (stage === 3 && state.form) {
            if (denied) {
                if (!(state.form.form.fields || []).some(f => f.key === "permission" && f.control.type === "copy"))
                    throw Error("Missing permission command")
                console.log("SETUP PASS permission denial is actionable")
                command("disable",{id:pluginId}); stage=6; return
            }
            if (state.form.wizard.step !== 2) throw Error("Firefox was not detected")
            test.contentItem.grabToImage(result => result.saveToFile(SCREENSHOT))
            click("pluginAction_login"); stage=4; return
        }
        if (stage === 4 && state.form?.status?.phase === "ready") {
            console.log("SETUP automatic Firefox return and account activation")
            stage=5; return
        }
        if (stage === 5 && state.form?.status?.phase === "ready") {
            console.log("SETUP PASS native wizard login credentials")
            command("disable",{id:pluginId}); stage=6; return
        }
        if (stage === 6 && (state.installed || []).some(p => p.id === pluginId && !p.enabled)) {
            console.log("SETUP PASS disabled")
            finish.start(); stage=7
        }
    }
    Timer { interval:100; running:true; repeat:true; onTriggered: {try {test.advance()} catch(e) {console.error("SETUP FAIL",String(e));Qt.exit(1)}} }
    Timer { id:finish; interval:1500; onTriggered:Qt.exit(0) }
    Timer { interval:100000; running:true; onTriggered: {console.error("SETUP FAIL timeout",test.stage);Qt.exit(1)} }
}
