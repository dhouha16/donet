// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
import { partition } from "../../../test_util/std/collections/partition.ts";
import { join } from "../../../test_util/std/path/mod.ts";

/**
 * The test suite matches the folders inside the `test` folder inside the
 * node repo
 *
 * Each test suite contains a list of files (which can be paths
 * or a regex to match) that will be pulled from the node repo
 */
type TestSuites = Record<string, string[]>;

interface Config {
  nodeVersion: string;
  /** Ignored files won't regenerated by the update script */
  ignore: TestSuites;
  /**
   * The files that will be run by the test suite
   *
   * The files to be generated with the update script must be listed here as well,
   * but they won't be regenerated if they are listed in the `ignore` configuration
   */
  tests: TestSuites;
  windowsIgnore: TestSuites;
  darwinIgnore: TestSuites;
}

export const config: Config = JSON.parse(
  await Deno.readTextFile(new URL("./config.json", import.meta.url)),
);

export const ignoreList = Object.entries(config.ignore).reduce(
  (total: RegExp[], [suite, paths]) => {
    paths.forEach((path) => total.push(new RegExp(join(suite, path))));
    return total;
  },
  [/package\.json/],
);

export function getPathsFromTestSuites(suites: TestSuites): string[] {
  const testPaths: string[] = [];
  for (const [dir, paths] of Object.entries(suites)) {
    if (
      ["parallel", "internet", "pummel", "sequential", "pseudo-tty"].includes(
        dir,
      )
    ) {
      for (const path of paths) {
        testPaths.push(join(dir, path));
      }
    }
  }
  return testPaths;
}

const PARALLEL_PATTERN = Deno.build.os == "windows"
  ? /^parallel[/\/]/
  : /^parallel\//;

export function partitionParallelTestPaths(
  testPaths: string[],
): { parallel: string[]; sequential: string[] } {
  const partitions = partition(testPaths, (p) => !!p.match(PARALLEL_PATTERN));
  return { parallel: partitions[0], sequential: partitions[1] };
}
