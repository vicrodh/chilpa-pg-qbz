import QtQuick
import QtTest
import @HOST_SOURCES_MODULE@
import "@HOST_PATTERNS@"

TestCase {
  name: "PluginPlaylistEditing"
  when: windowShown
  width: 1000
  height: 900
  visible: true
  SourcePlaylistEditor { id: editor }
  function init() {
    Sources.asked = []
    Sources.show({native_actions:{}})
    editor.begin({plugin:"fixture",ref:"list:1",title:"Name",editable:true,
      entries:[{ref:"song:7",member:"81",title:"Song"}]}, "update")
    tryCompare(editor, "visible", true)
  }
  function cleanup() { editor.close() }
  function answer(pending, error, result) {
    Sources.show({native_actions:{fixture:{request_id:editor.request,
      pending:pending,error:error,result:result}}})
  }
  function test_membership_identity_and_pending_write() {
    editor.submit("remove", {members:["81"]})
    const sent = JSON.parse(Sources.asked[0][2])
    compare(sent.action, "playlist.remove")
    compare(sent.ref, "list:1")
    compare(sent.members[0], "81")
    answer(true, "", {})
    verify(editor.pending)
    editor.submit("remove", {members:["81"]})
    compare(Sources.asked.length, 1)
  }
  function test_error_retains_editor_and_success_closes() {
    editor.submit("remove", {members:["81"]})
    answer(false, "Denied", {})
    compare(editor.error, "Denied")
    verify(editor.visible)
    editor.submit("update", {title:"Changed"})
    answer(false, "", {changed_refs:["list:1"]})
    tryCompare(editor, "visible", false)
  }
}
