// Source: https://github.com/xtermjs/xterm.js/
// Copyright (c) 2019, The xterm.js authors (https://github.com/xtermjs/xterm.js)
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

"use strict";
var MINIMUM_COLS = 2;
var MINIMUM_ROWS = 1;
function FitAddon() {
}
FitAddon.prototype.activate = function (terminal) {
    this._terminal = terminal;
};
FitAddon.prototype.dispose = function () { };
FitAddon.prototype.fit = function () {
    var dims = this.proposeDimensions();
    if (!dims || !this._terminal) {
        return;
    }
    var core = this._terminal._core;
    if (this._terminal.rows !== dims.rows || this._terminal.cols !== dims.cols) {
        core._renderService.clear();
        this._terminal.resize(dims.cols, dims.rows);
    }
};
FitAddon.prototype.proposeDimensions = function () {
    if (!this._terminal) {
        return undefined;
    }
    if (!this._terminal.element || !this._terminal.element.parentElement) {
        return undefined;
    }
    var core = this._terminal._core;
    if (core._renderService.dimensions.actualCellWidth === 0 || core._renderService.dimensions.actualCellHeight === 0) {
        return undefined;
    }
    var parentElementStyle = window.getComputedStyle(this._terminal.element.parentElement);
    var parentElementHeight = parseInt(parentElementStyle.getPropertyValue('height'));
    var parentElementWidth = Math.max(0, parseInt(parentElementStyle.getPropertyValue('width')));
    var elementStyle = window.getComputedStyle(this._terminal.element);
    var elementPadding = {
        top: parseInt(elementStyle.getPropertyValue('padding-top')),
        bottom: parseInt(elementStyle.getPropertyValue('padding-bottom')),
        right: parseInt(elementStyle.getPropertyValue('padding-right')),
        left: parseInt(elementStyle.getPropertyValue('padding-left'))
    };
    var elementPaddingVer = elementPadding.top + elementPadding.bottom;
    var elementPaddingHor = elementPadding.right + elementPadding.left;
    var availableHeight = parentElementHeight - elementPaddingVer;
    var availableWidth = parentElementWidth - elementPaddingHor - core.viewport.scrollBarWidth;
    var geometry = {
        cols: Math.max(MINIMUM_COLS, Math.floor(availableWidth / core._renderService.dimensions.actualCellWidth)),
        rows: Math.max(MINIMUM_ROWS, Math.floor(availableHeight / core._renderService.dimensions.actualCellHeight))
    };
    return geometry;
};
