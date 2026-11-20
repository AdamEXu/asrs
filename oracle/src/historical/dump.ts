import { encode } from "../cve1";

export interface DumpHeader {
  phase: string;
  status: string;
  loadingFields: string[];
}

/**
 * The canonical `Log` dump: CVE-1 lines, read from `Log.toSerialized()` so every value is the
 * one the page holds.
 *
 *   {"phase", "status", "loadingFields", "fieldCount"}
 *   one line per field in getFieldKeys() order: {"key", "type", "timestamps", "values",
 *     "structuredType", "wpilibType", "metadataString", "stripingReference", "typeWarning"}
 *   {"generatedParents"}
 *   {"timestampRange", "timestampZero"}
 *   {"structSchemaStrings", "structSchemas"}
 *   {"protoDescriptors"}
 *   {"queuedStructs", "queuedStructArrays", "queuedProtos"}
 */
export function dumpLog(serialized: any, header: DumpHeader): string[] {
  const keys = Object.keys(serialized.fields);
  const lines = [
    encode({
      phase: header.phase,
      status: header.status,
      loadingFields: header.loadingFields,
      fieldCount: keys.length
    })
  ];
  for (const key of keys) {
    const field = serialized.fields[key];
    lines.push(
      encode({
        key,
        type: field.type,
        timestamps: field.timestamps,
        values: field.values,
        structuredType: field.structuredType,
        wpilibType: field.wpilibType,
        metadataString: field.metadataString,
        stripingReference: field.stripingReference,
        typeWarning: field.typeWarning
      })
    );
  }
  lines.push(encode({ generatedParents: serialized.generatedParents }));
  lines.push(encode({ timestampRange: serialized.timestampRange, timestampZero: serialized.timestampZero }));
  lines.push(
    encode({
      structSchemaStrings: serialized.structDecoder.schemaStrings,
      structSchemas: serialized.structDecoder.schemas
    })
  );
  lines.push(encode({ protoDescriptors: serialized.protoDecoder }));
  lines.push(
    encode({
      queuedStructs: serialized.queuedStructs,
      queuedStructArrays: serialized.queuedStructArrays,
      queuedProtos: serialized.queuedProtos
    })
  );
  return lines;
}
