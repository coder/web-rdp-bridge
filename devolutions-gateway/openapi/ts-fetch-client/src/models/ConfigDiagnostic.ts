/* tslint:disable */
/* eslint-disable */
/**
 * devolutions-gateway
 * Protocol-aware fine-grained relay server
 *
 * The version of the OpenAPI document: 2022.2.2
 * Contact: infos@devolutions.net
 *
 * NOTE: This class is auto generated by OpenAPI Generator (https://openapi-generator.tech).
 * https://openapi-generator.tech
 * Do not edit the class manually.
 */

import { exists, mapValues } from '../runtime';
import type { ListenerUrls } from './ListenerUrls';
import {
    ListenerUrlsFromJSON,
    ListenerUrlsFromJSONTyped,
    ListenerUrlsToJSON,
} from './ListenerUrls';

/**
 * Service configuration diagnostic
 * @export
 * @interface ConfigDiagnostic
 */
export interface ConfigDiagnostic {
    /**
     * This Gateway's hostname
     * @type {string}
     * @memberof ConfigDiagnostic
     */
    hostname: string;
    /**
     * This Gateway's unique ID
     * @type {string}
     * @memberof ConfigDiagnostic
     */
    id?: string;
    /**
     * 
     * @type {Array<ListenerUrls>}
     * @memberof ConfigDiagnostic
     */
    listeners: Array<ListenerUrls>;
    /**
     * Gateway service version
     * @type {string}
     * @memberof ConfigDiagnostic
     */
    version: string;
}

/**
 * Check if a given object implements the ConfigDiagnostic interface.
 */
export function instanceOfConfigDiagnostic(value: object): boolean {
    let isInstance = true;
    isInstance = isInstance && "hostname" in value;
    isInstance = isInstance && "listeners" in value;
    isInstance = isInstance && "version" in value;

    return isInstance;
}

export function ConfigDiagnosticFromJSON(json: any): ConfigDiagnostic {
    return ConfigDiagnosticFromJSONTyped(json, false);
}

export function ConfigDiagnosticFromJSONTyped(json: any, ignoreDiscriminator: boolean): ConfigDiagnostic {
    if ((json === undefined) || (json === null)) {
        return json;
    }
    return {
        
        'hostname': json['hostname'],
        'id': !exists(json, 'id') ? undefined : json['id'],
        'listeners': ((json['listeners'] as Array<any>).map(ListenerUrlsFromJSON)),
        'version': json['version'],
    };
}

export function ConfigDiagnosticToJSON(value?: ConfigDiagnostic | null): any {
    if (value === undefined) {
        return undefined;
    }
    if (value === null) {
        return null;
    }
    return {
        
        'hostname': value.hostname,
        'id': value.id,
        'listeners': ((value.listeners as Array<any>).map(ListenerUrlsToJSON)),
        'version': value.version,
    };
}

