// Independently authored TypeSpec authority for the actual web/API transport identity.
// JSON Schema A remains a separate human-authored peer.
import "@typespec/json-schema";
using TypeSpec.JsonSchema;
namespace OresWebApiPlane;

@id("WebApiPlaneBinding")
model WebApiPlaneBinding {
  githubOrg: "anticaptrad";
  orgSlug: "anticaptrad";
  dnsZone: "anticaptrad.dev";
  apiHost: "api.anticaptrad.dev";
  natsRequestSubject: "dd.remote.web_api.anticaptrad.request";
}
